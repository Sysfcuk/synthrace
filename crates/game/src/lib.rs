//! SYNTHRACE browser entry point (M0 spike): wgpu with WebGPU→WebGL2 fallback,
//! RAF loop, keyboard/touch input, AudioWorklet synth and a debug overlay.
#![cfg(target_arch = "wasm32")]

mod audio;

use audio::Audio;
use render::{FrameParams, Renderer, demo};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlCanvasElement, HtmlElement, KeyboardEvent, PointerEvent};

const TOP_SPEED_MS: f32 = 85.0; // ≈ 306 km/h at speed01 = 1
const CENTRIFUGAL: f32 = 7000.0;

#[derive(Default)]
struct Input {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    touch_steer: f32,
    touching: bool,
    any_input: bool,
}

struct App {
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    canvas: HtmlCanvasElement,
    backend: String,
    adapter_name: String,
    input: Rc<RefCell<Input>>,
    audio: Rc<RefCell<Option<Audio>>>,
    muted: Rc<RefCell<bool>>,
    t: f64,
    last: f64,
    speed: f32,
    scroll: f32,
    x: f32,
    frame_ms: Vec<f32>,
    hud_timer: f64,
    hud: HtmlElement,
    last_speed_sent: f32,
}

fn window() -> web_sys::Window {
    web_sys::window().expect("no window")
}

fn el(id: &str) -> Option<HtmlElement> {
    window().document()?.get_element_by_id(id)?.dyn_into().ok()
}

fn query(name: &str) -> Option<String> {
    let search = window().location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&search).ok()?.get(name)
}

fn fatal(msg: &str) {
    if let Some(f) = el("fatal") {
        f.set_inner_text(msg);
        let _ = f.style().set_property("display", "flex");
    }
    web_sys::console::error_1(&msg.into());
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = run().await {
            fatal(&format!("SYNTHRACE could not start: {e}\n\nA browser with WebGPU or WebGL2 is required."));
        }
    });
}

async fn run() -> Result<(), String> {
    let canvas: HtmlCanvasElement = el("game").ok_or("no canvas")?.dyn_into().map_err(|_| "not a canvas")?;
    let force_gl = query("backend").as_deref() == Some("gl");

    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    let instance = if force_gl {
        desc.backends = wgpu::Backends::GL;
        wgpu::Instance::new(desc)
    } else {
        desc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        wgpu::util::new_instance_with_webgpu_detection(desc).await
    };
    let (w, h) = canvas_size(&canvas);
    canvas.set_width(w);
    canvas.set_height(h);
    let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone())).map_err(|e| format!("surface: {e}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("adapter: {e}"))?;
    let info = adapter.get_info();
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor { label: Some("synthrace"), required_limits: limits, ..Default::default() })
        .await
        .map_err(|e| format!("device: {e}"))?;
    device.on_uncaptured_error(std::sync::Arc::new(|e| web_sys::console::error_1(&format!("wgpu: {e}").into())));

    let mut config = surface.get_default_config(&adapter, w, h).ok_or("surface unsupported by adapter")?;
    config.present_mode = wgpu::PresentMode::Fifo;
    surface.configure(&device, &config);

    let hdr = render::choose_hdr_format(&adapter);
    let renderer = Renderer::new(device, queue, hdr, config.format, w, h);
    let backend = format!("{:?}", info.backend);
    log_line(&format!("SYNTHRACE M0: backend={backend} adapter='{}' surface={:?} hdr={hdr:?}", info.name, config.format));

    let input = Rc::new(RefCell::new(Input::default()));
    let audio: Rc<RefCell<Option<Audio>>> = Rc::new(RefCell::new(None));
    let muted = Rc::new(RefCell::new(false));
    install_input(&canvas, input.clone(), audio.clone(), muted.clone());
    install_start_button(audio.clone(), input.clone());
    install_visibility(audio.clone());

    let now = window().performance().map(|p| p.now()).unwrap_or(0.0);
    let app = Rc::new(RefCell::new(App {
        renderer,
        surface,
        config,
        canvas,
        backend,
        adapter_name: info.name,
        input,
        audio,
        muted,
        t: 0.0,
        last: now,
        speed: 0.45,
        scroll: 0.0,
        x: 0.0,
        frame_ms: Vec::with_capacity(128),
        hud_timer: 0.0,
        hud: el("hud").ok_or("no hud")?,
        last_speed_sent: -1.0,
    }));
    start_loop(app);
    Ok(())
}

fn canvas_size(canvas: &HtmlCanvasElement) -> (u32, u32) {
    let dpr = window().device_pixel_ratio().min(2.0);
    let scale: f64 = query("scale").and_then(|s| s.parse().ok()).unwrap_or(1.0f64).clamp(0.25, 2.0);
    let w = (canvas.client_width() as f64 * dpr * scale).round().max(1.0) as u32;
    let h = (canvas.client_height() as f64 * dpr * scale).round().max(1.0) as u32;
    (w.min(4096), h.min(4096))
}

fn log_line(s: &str) {
    web_sys::console::log_1(&s.into());
}

fn start_loop(app: Rc<RefCell<App>>) {
    type RafSlot = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
    let f: RafSlot = Rc::new(RefCell::new(None));
    let g = f.clone();
    *g.borrow_mut() = Some(Closure::new(move |ts: f64| {
        app.borrow_mut().frame(ts);
        let _ = window().request_animation_frame(f.borrow().as_ref().unwrap().as_ref().unchecked_ref());
    }));
    let _ = window().request_animation_frame(g.borrow().as_ref().unwrap().as_ref().unchecked_ref());
}

impl App {
    fn frame(&mut self, ts: f64) {
        let dt = ((ts - self.last) / 1000.0).clamp(0.0, 0.1) as f32;
        self.frame_ms.push((ts - self.last) as f32);
        self.last = ts;
        self.t += dt as f64;

        // --- spike "driving": arcade speed + steering + centrifugal push ---
        {
            let i = self.input.borrow();
            let accel = i.up || (i.touching) || !i.any_input;
            if accel && !i.down {
                let target = if i.any_input { 1.0 } else { 0.55 };
                self.speed += (target - self.speed).signum() * 0.22 * dt * (1.0 - self.speed * 0.6);
            } else if i.down {
                self.speed -= 0.6 * dt;
            } else {
                self.speed -= 0.05 * dt;
            }
            self.speed = self.speed.clamp(0.0, 1.0);
            let mut steer = (i.right as i32 - i.left as i32) as f32;
            if i.touching {
                steer = i.touch_steer;
            }
            let steer_rate = 11.0 * (0.45 + 0.55 * (1.0 - self.speed));
            if !i.any_input {
                // Attract: cancel the centrifugal push and hold a lane (P-controller).
                let push = demo::curve(self.scroll) * self.speed * self.speed * CENTRIFUGAL;
                steer = ((push + (2.0 - self.x) * 1.5) / steer_rate).clamp(-1.0, 1.0);
            }
            self.x += steer * steer_rate * dt;
            self.x -= demo::curve(self.scroll) * self.speed * self.speed * CENTRIFUGAL * dt;
            self.x = self.x.clamp(-11.0, 11.0);
        }
        self.scroll += self.speed * TOP_SPEED_MS * dt;

        // --- resize ---
        let (w, h) = canvas_size(&self.canvas);
        if w != self.config.width || h != self.config.height {
            self.canvas.set_width(w);
            self.canvas.set_height(h);
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(self.renderer.device(), &self.config);
            self.renderer.resize(w, h);
        }

        // --- audio params + beat clock ---
        let beat_phase = match self.audio.borrow().as_ref() {
            Some(a) => {
                if (self.speed - self.last_speed_sent).abs() > 0.01 {
                    a.set_param(dsp_param::SPEED, self.speed);
                    self.last_speed_sent = self.speed;
                }
                a.beat_phase()
            }
            None => ((self.t * 118.0 / 60.0).fract()) as f32,
        };

        let params = FrameParams {
            time: self.t as f32,
            scroll: self.scroll,
            speed01: self.speed,
            beat_phase,
            player_x: self.x,
            profile: demo::profile(self.scroll),
        };
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                self.surface.configure(self.renderer.device(), &self.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        self.renderer.render(&view, &params);
        self.renderer.queue().present(frame);

        self.hud_timer += dt as f64;
        if self.hud_timer > 0.5 {
            self.hud_timer = 0.0;
            self.update_hud();
        }
    }

    fn update_hud(&mut self) {
        let mut v = std::mem::take(&mut self.frame_ms);
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let (p50, p95) = if v.is_empty() { (0.0, 0.0) } else { (v[v.len() / 2], v[((v.len() - 1) as f32 * 0.95) as usize]) };
        let audio = match self.audio.borrow().as_ref() {
            Some(a) => format!("{} @ {} Hz{}", a.state(), a.sample_rate(), if *self.muted.borrow() { " (muted)" } else { "" }),
            None => "not started".into(),
        };
        let s = self.renderer.stats;
        let (rw, rh) = self.renderer.size();
        self.hud.set_inner_text(&format!(
            "SYNTHRACE M0 spike\nbackend {}  ·  {}\nframe p50 {:.1} ms  p95 {:.1} ms  ({:.0} fps)\nres {}x{}  draws {}  passes {}\nhdr {:?}\nspeed {:>3.0} km/h\naudio {}",
            self.backend,
            self.adapter_name,
            p50,
            p95,
            if p50 > 0.0 { 1000.0 / p50 } else { 0.0 },
            rw,
            rh,
            s.draw_calls,
            s.passes,
            self.renderer.hdr_format(),
            self.speed * TOP_SPEED_MS * 3.6,
            audio
        ));
        self.frame_ms = v;
        self.frame_ms.clear();
    }
}

mod dsp_param {
    // Mirrors dsp::param (kept local to avoid linking the DSP into the game module).
    pub const SPEED: u32 = 1;
    pub const MUTE: u32 = 2;
}

fn install_input(
    canvas: &HtmlCanvasElement,
    input: Rc<RefCell<Input>>,
    audio: Rc<RefCell<Option<Audio>>>,
    muted: Rc<RefCell<bool>>,
) {
    let w = window();
    let inp = input.clone();
    let down = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
        let mut i = inp.borrow_mut();
        let handled = match e.code().as_str() {
            "ArrowLeft" | "KeyA" => {
                i.left = true;
                true
            }
            "ArrowRight" | "KeyD" => {
                i.right = true;
                true
            }
            "ArrowUp" | "KeyW" => {
                i.up = true;
                true
            }
            "ArrowDown" | "KeyS" => {
                i.down = true;
                true
            }
            "KeyM" => {
                if !e.repeat() {
                    let mut m = muted.borrow_mut();
                    *m = !*m;
                    if let Some(a) = audio.borrow().as_ref() {
                        a.set_param(dsp_param::MUTE, if *m { 1.0 } else { 0.0 });
                    }
                }
                true
            }
            _ => false,
        };
        if handled {
            i.any_input = true;
            e.prevent_default();
        }
    });
    let inp = input.clone();
    let up = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
        let mut i = inp.borrow_mut();
        match e.code().as_str() {
            "ArrowLeft" | "KeyA" => i.left = false,
            "ArrowRight" | "KeyD" => i.right = false,
            "ArrowUp" | "KeyW" => i.up = false,
            "ArrowDown" | "KeyS" => i.down = false,
            _ => {}
        }
    });
    let _ = w.add_event_listener_with_callback("keydown", down.as_ref().unchecked_ref());
    let _ = w.add_event_listener_with_callback("keyup", up.as_ref().unchecked_ref());
    down.forget();
    up.forget();

    // Touch / pointer: hold to accelerate, horizontal position steers.
    let c = canvas.clone();
    let inp = input.clone();
    let pdown = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
        let mut i = inp.borrow_mut();
        i.touching = true;
        i.any_input = true;
        let width = c.client_width().max(1) as f32;
        i.touch_steer = ((e.client_x() as f32 / width) * 2.0 - 1.0).clamp(-1.0, 1.0) * 1.4;
        let _ = c.set_pointer_capture(e.pointer_id());
    });
    let c2 = canvas.clone();
    let inp = input.clone();
    let pmove = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
        let mut i = inp.borrow_mut();
        if i.touching {
            let width = c2.client_width().max(1) as f32;
            i.touch_steer = ((e.client_x() as f32 / width) * 2.0 - 1.0).clamp(-1.0, 1.0) * 1.4;
        }
    });
    let inp = input;
    let pup = Closure::<dyn FnMut(PointerEvent)>::new(move |_e: PointerEvent| {
        let mut i = inp.borrow_mut();
        i.touching = false;
        i.touch_steer = 0.0;
    });
    let _ = canvas.add_event_listener_with_callback("pointerdown", pdown.as_ref().unchecked_ref());
    let _ = canvas.add_event_listener_with_callback("pointermove", pmove.as_ref().unchecked_ref());
    let _ = canvas.add_event_listener_with_callback("pointerup", pup.as_ref().unchecked_ref());
    let _ = canvas.add_event_listener_with_callback("pointercancel", pup.as_ref().unchecked_ref());
    pdown.forget();
    pmove.forget();
    pup.forget();
}

fn install_start_button(audio: Rc<RefCell<Option<Audio>>>, input: Rc<RefCell<Input>>) {
    let Some(button) = el("start") else { return };
    let b = button.clone();
    let on_click = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        let _ = b.class_list().add_1("hidden");
        if let Some(c) = el("game") {
            let _ = c.focus();
        }
        input.borrow_mut().any_input = false;
        if audio.borrow().is_some() {
            return;
        }
        // The AudioContext must be created synchronously inside the gesture.
        match Audio::create_context() {
            Ok(ctx) => {
                let slot = audio.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match Audio::start(ctx).await {
                        Ok(a) => {
                            log_line(&format!("audio: worklet running at {} Hz", a.sample_rate()));
                            *slot.borrow_mut() = Some(a);
                        }
                        Err(e) => web_sys::console::warn_2(&"audio unavailable:".into(), &e),
                    }
                });
            }
            Err(e) => web_sys::console::warn_2(&"audio unavailable:".into(), &e),
        }
    });
    let _ = button.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref());
    on_click.forget();
}

fn install_visibility(audio: Rc<RefCell<Option<Audio>>>) {
    let doc = window().document().expect("document");
    let d2 = doc.clone();
    let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        if let Some(a) = audio.borrow().as_ref() {
            if d2.hidden() { a.suspend() } else { a.resume() }
        }
    });
    let _ = doc.add_event_listener_with_callback("visibilitychange", cb.as_ref().unchecked_ref());
    cb.forget();
}
