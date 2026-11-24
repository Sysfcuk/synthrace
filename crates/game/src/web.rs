//! SYNTHRACE browser entry point: wgpu with WebGPU→WebGL2 fallback, RAF loop driving a
//! fixed-step simulation, input, AudioWorklet synth, minimal HUD and a debug overlay.

use crate::audio::Audio;
use crate::input::Input;
use crate::scene::Scene;
use crate::vfx::Vfx;
use render::Renderer;
use sim::{DT, SimEvent, TUNING, World};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlCanvasElement, HtmlElement};

const MAX_STEPS_PER_FRAME: u32 = 8;
/// Traffic voices are assigned to the nearest cars within this range (m).
const TRAFFIC_AUDIO_RANGE: f32 = 140.0;

mod dsp_param {
    // Mirrors dsp::param (kept local to avoid linking the DSP into the game module).
    pub const SPEED: u32 = 1;
    pub const MUTE: u32 = 2;
    pub const RPM: u32 = 3;
    pub const TRAFFIC: u32 = 7;
}

mod shot {
    // Mirrors dsp::sfx::shot.
    pub const IMPACT: u32 = 0;
    pub const WHOOSH: u32 = 1;
    pub const NITRO: u32 = 2;
    pub const SHIFT: u32 = 3;
    pub const BLIP: u32 = 4;
}

struct Hud {
    debug: HtmlElement,
    speed: Option<HtmlElement>,
    gear: Option<HtmlElement>,
    nitro: Option<HtmlElement>,
    callout: Option<HtmlElement>,
    show_debug: bool,
    callout_timer: f32,
}

struct App {
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    canvas: HtmlCanvasElement,
    backend: String,
    adapter_name: String,
    input: Input,
    audio: Rc<RefCell<Option<Audio>>>,
    muted: bool,
    t: f64,
    last: f64,
    world: World,
    scene: Scene,
    vfx: Vfx,
    acc: f32,
    frame_ms: Vec<f32>,
    hud_timer: f64,
    hud: Hud,
    last_speed_sent: f32,
    /// Near misses in the current streak (drives the key-locked blip pitch).
    streak: u32,
    streak_timer: f32,
    near_misses: u32,
    overtakes: u32,
    takedowns: u32,
}

fn window() -> web_sys::Window {
    web_sys::window().expect("no window")
}

pub(crate) fn el(id: &str) -> Option<HtmlElement> {
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

/// Determinism self-test for the e2e suite: must equal `sim::GOLDEN_HASH_SEED42_60S`
/// (the native value) for the wasm build to be considered deterministic.
#[wasm_bindgen]
pub fn sim_probe_hash() -> String {
    format!("{:#018x}", sim::determinism_probe(42, 60))
}

#[wasm_bindgen]
pub fn sim_golden_hash() -> String {
    format!("{:#018x}", sim::GOLDEN_HASH_SEED42_60S)
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
    log_line(&format!("SYNTHRACE boot: backend={backend} adapter='{}' surface={:?} hdr={hdr:?}", info.name, config.format));

    let input = Input::install(&canvas);
    let audio: Rc<RefCell<Option<Audio>>> = Rc::new(RefCell::new(None));
    install_start_button(audio.clone(), input.state.clone());
    install_visibility(audio.clone());

    let now = window().performance().map(|p| p.now()).unwrap_or(0.0);
    let show_debug = query("debug").map(|v| v != "0").unwrap_or(true);
    let debug = el("hud").ok_or("no hud")?;
    if !show_debug {
        let _ = debug.style().set_property("display", "none");
    }
    let app = Rc::new(RefCell::new(App {
        renderer,
        surface,
        config,
        canvas,
        backend,
        adapter_name: info.name,
        input,
        audio,
        muted: false,
        t: 0.0,
        last: now,
        world: World::new(query("seed").and_then(|s| s.parse().ok()).unwrap_or(1986)),
        scene: Scene::new(),
        vfx: Vfx::new(),
        acc: 0.0,
        frame_ms: Vec::with_capacity(128),
        hud_timer: 0.0,
        hud: Hud {
            debug,
            speed: el("hud-speed"),
            gear: el("hud-gear"),
            nitro: el("nitro-fill"),
            callout: el("callout"),
            show_debug,
            callout_timer: 0.0,
        },
        last_speed_sent: -1.0,
        streak: 0,
        streak_timer: 0.0,
        near_misses: 0,
        overtakes: 0,
        takedowns: 0,
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
        self.handle_toggles();

        // --- input → fixed-step simulation (120 Hz), paused during hit-stop ---
        let input = self.input.poll().unwrap_or_else(|| sim::autopilot(&self.world, sim::LANES[2]));
        if self.vfx.hitstop <= 0.0 {
            self.acc += dt;
        }
        let mut steps = 0;
        while self.acc >= DT && steps < MAX_STEPS_PER_FRAME {
            self.world.step(&input);
            self.acc -= DT;
            steps += 1;
        }
        if steps == MAX_STEPS_PER_FRAME {
            self.acc = 0.0; // drop time after a long hitch rather than spiral
        }
        let events: Vec<SimEvent> = std::mem::take(&mut self.world.events);
        for e in &events {
            self.on_event(e);
        }
        self.vfx.update(&self.world, dt);
        self.streak_timer -= dt;
        if self.streak_timer <= 0.0 {
            self.streak = 0;
        }
        let alpha = (self.acc / DT).clamp(0.0, 1.0);

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

        let beat_phase = self.feed_audio(&input);
        let params = self.scene.build(&self.world, &mut self.vfx, alpha, self.t as f32, beat_phase, dt);
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

        self.update_hud(dt);
    }

    fn handle_toggles(&mut self) {
        let (mute, debug) = {
            let mut s = self.input.state.borrow_mut();
            let r = (s.mute_toggled, s.debug_toggled);
            s.mute_toggled = false;
            s.debug_toggled = false;
            r
        };
        if mute {
            self.muted = !self.muted;
            if let Some(a) = self.audio.borrow().as_ref() {
                a.set_param(dsp_param::MUTE, if self.muted { 1.0 } else { 0.0 });
            }
        }
        if debug {
            self.hud.show_debug = !self.hud.show_debug;
            let _ = self.hud.debug.style().set_property("display", if self.hud.show_debug { "block" } else { "none" });
        }
    }

    fn callout(&mut self, text: &str, class: &str) {
        if let Some(c) = &self.hud.callout {
            c.set_inner_text(text);
            c.set_class_name("");
            let _ = c.offset_width(); // restart the CSS animation
            c.set_class_name(&format!("show {class}"));
            self.hud.callout_timer = 0.9;
        }
    }

    fn on_event(&mut self, e: &SimEvent) {
        self.vfx.on_event(e, &self.world);
        let audio = self.audio.borrow();
        let a = audio.as_ref();
        match *e {
            SimEvent::NearMiss { close, side, .. } => {
                self.near_misses += 1;
                self.streak += 1;
                self.streak_timer = 3.0;
                if let Some(a) = a {
                    a.trigger(shot::WHOOSH, side);
                    a.trigger(shot::BLIP, (self.streak - 1) as f32);
                }
                drop(audio);
                if close {
                    self.callout("CLOSE!", "hot")
                } else {
                    self.callout("NEAR MISS", "cool")
                }
                self.input.rumble(0.0, if close { 0.5 } else { 0.25 }, 80.0);
            }
            SimEvent::Overtake => self.overtakes += 1,
            SimEvent::Takedown { .. } => {
                self.takedowns += 1;
                if let Some(a) = a {
                    a.trigger(shot::IMPACT, 0.9);
                    a.trigger(shot::BLIP, 6.0);
                }
                drop(audio);
                self.callout("TAKEDOWN!", "hot");
                self.input.rumble(0.7, 0.7, 250.0);
            }
            SimEvent::TrafficImpact { ds, strength, .. } => {
                if let Some(a) = a {
                    let fall = (1.0 - ds.abs() / 150.0).max(0.0);
                    if fall > 0.05 {
                        a.trigger(shot::IMPACT, strength * fall);
                    }
                }
            }
            SimEvent::Bump { strength, .. } => {
                if let Some(a) = a {
                    a.trigger(shot::IMPACT, strength);
                }
                drop(audio);
                self.input.rumble(0.6 * strength, 0.4, 160.0);
            }
            SimEvent::SpinOut | SimEvent::Crash => {
                if let Some(a) = a {
                    a.trigger(shot::IMPACT, 1.0);
                }
                drop(audio);
                self.callout(if *e == SimEvent::Crash { "CRASH" } else { "SPIN OUT" }, "warn");
                self.input.rumble(1.0, 1.0, 400.0);
            }
            SimEvent::Scrape { .. } => {
                if let Some(a) = a {
                    a.trigger(shot::IMPACT, 0.35);
                }
                drop(audio);
                self.input.rumble(0.3, 0.6, 120.0);
            }
            SimEvent::DriftTier { tier } => {
                if let Some(a) = a {
                    a.trigger(shot::BLIP, (tier * 2) as f32);
                }
                drop(audio);
                self.callout(&format!("DRIFT {}", ["", "I", "II", "III"][tier as usize]), "cool");
            }
            SimEvent::DriftBoost { tier } => {
                drop(audio);
                self.callout(&format!("BOOST {}", ["", "I", "II", "III"][tier as usize]), "hot");
            }
            SimEvent::NitroStart => {
                if let Some(a) = a {
                    a.trigger(shot::NITRO, 1.0);
                }
                drop(audio);
                self.callout("NITRO", "hot");
            }
            SimEvent::GearShift { .. } => {
                if let Some(a) = a {
                    a.trigger(shot::SHIFT, 1.0);
                }
            }
            _ => {}
        }
    }

    /// Sends engine/tyre/traffic parameters; returns the beat phase for visuals.
    fn feed_audio(&mut self, input: &sim::Input) -> f32 {
        let audio = self.audio.borrow();
        let Some(a) = audio.as_ref() else { return ((self.t * 118.0 / 60.0).fract()) as f32 };
        let c = &self.world.car;
        let speed01 = c.v / TUNING.v_max;
        if (speed01 - self.last_speed_sent).abs() > 0.01 {
            a.set_param(dsp_param::SPEED, speed01);
            self.last_speed_sent = speed01;
        }
        let throttle = if c.stun > 0.0 { 0.0 } else { input.throttle };
        a.set_params(dsp_param::RPM, &[c.rpm, throttle, c.slip, if c.nitro_active { 1.0 } else { 0.0 }]);

        // Nearest four traffic cars → pass-by voices with doppler and pan.
        let track = &self.world.track;
        let mut near: Vec<(f32, &sim::TrafficCar)> = self
            .world
            .traffic
            .cars
            .iter()
            .map(|t| (track.delta(t.s, c.s), t))
            .filter(|(d, _)| d.abs() < TRAFFIC_AUDIO_RANGE)
            .collect();
        near.sort_by(|a, b| a.0.abs().total_cmp(&b.0.abs()));
        let mut vals = [0.0f32; 12];
        for (i, (ds, t)) in near.iter().take(4).enumerate() {
            let dist = (ds * ds + (t.x - c.x) * (t.x - c.x)).sqrt();
            let gain = (1.0 - dist / TRAFFIC_AUDIO_RANGE).max(0.0).powi(2);
            // Closing speed toward the listener (positive = approaching).
            let closing = (c.v - t.v) * ds.signum();
            let doppler = (343.0 / (343.0 - closing.clamp(-120.0, 120.0))).clamp(0.7, 1.45);
            let base = 45.0 + t.v * 2.4 + if t.kind == sim::Archetype::Truck { -20.0 } else { 0.0 };
            vals[i * 3] = gain;
            vals[i * 3 + 1] = base * doppler;
            vals[i * 3 + 2] = ((t.x - c.x) / 8.0).clamp(-1.0, 1.0);
        }
        a.set_params(dsp_param::TRAFFIC, &vals);
        a.beat_phase()
    }

    fn update_hud(&mut self, dt: f32) {
        let c = &self.world.car;
        if let Some(s) = &self.hud.speed {
            s.set_inner_text(&format!("{:.0}", c.v * 3.6));
        }
        if let Some(g) = &self.hud.gear {
            g.set_inner_text(&format!("{}", c.gear));
        }
        if let Some(n) = &self.hud.nitro {
            let _ = n.style().set_property("width", &format!("{:.1}%", c.nitro));
            n.set_class_name(if c.nitro_active { "burn" } else { "" });
        }
        if self.hud.callout_timer > 0.0 {
            self.hud.callout_timer -= dt;
        }

        self.hud_timer += dt as f64;
        if self.hud_timer < 0.5 || !self.hud.show_debug {
            return;
        }
        self.hud_timer = 0.0;
        let mut v = std::mem::take(&mut self.frame_ms);
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let (p50, p95) = if v.is_empty() { (0.0, 0.0) } else { (v[v.len() / 2], v[((v.len() - 1) as f32 * 0.95) as usize]) };
        let audio = match self.audio.borrow().as_ref() {
            Some(a) => format!("{} @ {} Hz{}", a.state(), a.sample_rate(), if self.muted { " (muted)" } else { "" }),
            None => "not started".into(),
        };
        let s = self.renderer.stats;
        let (rw, rh) = self.renderer.size();
        let input_dev = self.input.state.borrow().last_device;
        self.hud.debug.set_inner_text(&format!(
            "SYNTHRACE M2.5 · handling  (F3 hides)\nbackend {}  ·  {}\nframe p50 {:.1} ms  p95 {:.1} ms  ({:.0} fps)\nres {}x{}  draws {}  passes {}\ntraffic {}  particles {}\nnear misses {}  overtakes {}  takedowns {}\ninput {}  ·  audio {}",
            self.backend,
            self.adapter_name,
            p50,
            p95,
            if p50 > 0.0 { 1000.0 / p50 } else { 0.0 },
            rw,
            rh,
            s.draw_calls,
            s.passes,
            self.world.traffic.cars.len(),
            s.particles,
            self.near_misses,
            self.overtakes,
            self.takedowns,
            input_dev,
            audio
        ));
        self.frame_ms = v;
        self.frame_ms.clear();
    }
}

fn install_start_button(audio: Rc<RefCell<Option<Audio>>>, input: Rc<RefCell<crate::input::InputState>>) {
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
