//! Browser input: keyboard, pointer/touch (steer by finger position, on-screen buttons) and the
//! Gamepad API (standard mapping). Produces one `sim::Input` per frame.

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlCanvasElement, HtmlElement, KeyboardEvent, PointerEvent};

const STICK_DEADZONE: f32 = 0.12;

#[derive(Default)]
pub struct InputState {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    drift_key: bool,
    nitro_key: bool,
    /// Active steering pointer (finger or mouse button held on the canvas).
    steer_pointer: Option<i32>,
    touch_steer: f32,
    touch_mode: bool,
    touch_brake: bool,
    touch_drift: bool,
    touch_nitro: bool,
    pub any_input: bool,
    pub mute_toggled: bool,
    pub debug_toggled: bool,
    pub last_device: &'static str,
}

/// Gamepad snapshot for one frame.
#[derive(Default, Clone, Copy)]
struct Pad {
    steer: f32,
    throttle: f32,
    brake: f32,
    drift: bool,
    nitro: bool,
    active: bool,
}

fn button(buttons: &js_sys::Array, i: u32) -> (bool, f32) {
    let b: JsValue = buttons.get(i);
    match b.dyn_into::<web_sys::GamepadButton>() {
        Ok(b) => (b.pressed(), b.value() as f32),
        Err(_) => (false, 0.0),
    }
}

fn poll_pad() -> Option<(Pad, web_sys::Gamepad)> {
    let pads = web_sys::window()?.navigator().get_gamepads().ok()?;
    for p in pads.iter() {
        let Ok(pad) = p.dyn_into::<web_sys::Gamepad>() else { continue };
        if !pad.connected() {
            continue;
        }
        let axes = pad.axes();
        let buttons = pad.buttons();
        let raw = axes.get(0).as_f64().unwrap_or(0.0) as f32;
        // Deadzone + gentle response curve for fine control near centre.
        let stick =
            if raw.abs() < STICK_DEADZONE { 0.0 } else { (raw.abs() - STICK_DEADZONE) / (1.0 - STICK_DEADZONE) * raw.signum() };
        let stick = stick * (0.4 + 0.6 * stick.abs());
        let dpad = button(&buttons, 15).0 as i32 as f32 - button(&buttons, 14).0 as i32 as f32;
        let steer = if dpad != 0.0 { dpad } else { stick };
        let (_, rt) = button(&buttons, 7);
        let (_, lt) = button(&buttons, 6);
        let a = button(&buttons, 0).0;
        let rb = button(&buttons, 5).0;
        let x = button(&buttons, 2).0;
        let lb = button(&buttons, 4).0;
        let pad_state = Pad { steer, throttle: rt, brake: lt, drift: a || rb, nitro: x || lb, active: false };
        let active = steer != 0.0 || rt > 0.05 || lt > 0.05 || pad_state.drift || pad_state.nitro;
        return Some((Pad { active, ..pad_state }, pad));
    }
    None
}

pub struct Input {
    pub state: Rc<RefCell<InputState>>,
    pad: Option<web_sys::Gamepad>,
}

impl Input {
    pub fn install(canvas: &HtmlCanvasElement) -> Self {
        let state = Rc::new(RefCell::new(InputState { last_device: "keyboard", ..Default::default() }));
        install_keyboard(state.clone());
        install_pointer(canvas, state.clone());
        install_touch_buttons(state.clone());
        Self { state, pad: None }
    }

    /// Merged input for this frame, or `None` while nobody has touched a control (attract mode).
    pub fn poll(&mut self) -> Option<sim::Input> {
        let pad = poll_pad();
        let mut s = self.state.borrow_mut();
        let mut out = sim::Input::default();
        if let Some((p, handle)) = pad {
            if p.active {
                s.any_input = true;
                s.last_device = "gamepad";
            }
            self.pad = Some(handle);
            if s.last_device == "gamepad" {
                out = sim::Input { steer: p.steer, throttle: p.throttle, brake: p.brake, drift: p.drift, nitro: p.nitro };
            }
        }
        if !s.any_input {
            return None;
        }
        if s.last_device != "gamepad" {
            let keys_steer = (s.right as i32 - s.left as i32) as f32;
            let steer = if s.steer_pointer.is_some() { s.touch_steer } else { keys_steer };
            // Touch mode auto-accelerates unless braking; mouse/keyboard use the throttle key.
            let throttle = if s.up || (s.touch_mode && !s.touch_brake) { 1.0 } else { 0.0 };
            out = sim::Input {
                steer,
                throttle,
                brake: if s.down || s.touch_brake { 1.0 } else { 0.0 },
                drift: s.drift_key || s.touch_drift,
                nitro: s.nitro_key || s.touch_nitro,
            };
        }
        Some(out)
    }

    /// Short rumble on the active gamepad, if it supports it.
    pub fn rumble(&self, strong: f32, weak: f32, ms: f32) {
        let Some(pad) = &self.pad else { return };
        let Ok(act) = js_sys::Reflect::get(pad, &"vibrationActuator".into()) else { return };
        if act.is_undefined() || act.is_null() {
            return;
        }
        let params = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&params, &"duration".into(), &ms.into());
        let _ = js_sys::Reflect::set(&params, &"strongMagnitude".into(), &strong.clamp(0.0, 1.0).into());
        let _ = js_sys::Reflect::set(&params, &"weakMagnitude".into(), &weak.clamp(0.0, 1.0).into());
        if let Ok(f) = js_sys::Reflect::get(&act, &"playEffect".into())
            && let Ok(f) = f.dyn_into::<js_sys::Function>()
        {
            let _ = f.call2(&act, &"dual-rumble".into(), &params);
        }
    }
}

fn install_keyboard(state: Rc<RefCell<InputState>>) {
    let w = web_sys::window().expect("window");
    let st = state.clone();
    let down = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
        let mut s = st.borrow_mut();
        let handled = match e.code().as_str() {
            "ArrowLeft" | "KeyA" => {
                s.left = true;
                true
            }
            "ArrowRight" | "KeyD" => {
                s.right = true;
                true
            }
            "ArrowUp" | "KeyW" => {
                s.up = true;
                true
            }
            "ArrowDown" | "KeyS" => {
                s.down = true;
                true
            }
            "Space" | "ShiftLeft" | "ShiftRight" => {
                s.drift_key = true;
                true
            }
            "KeyN" | "ControlLeft" | "ControlRight" | "KeyX" => {
                s.nitro_key = true;
                true
            }
            "KeyM" => {
                if !e.repeat() {
                    s.mute_toggled = true;
                }
                return;
            }
            "F3" => {
                if !e.repeat() {
                    s.debug_toggled = true;
                }
                e.prevent_default();
                return;
            }
            _ => false,
        };
        if handled {
            s.any_input = true;
            s.last_device = "keyboard";
            e.prevent_default();
        }
    });
    let up = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
        let mut s = state.borrow_mut();
        match e.code().as_str() {
            "ArrowLeft" | "KeyA" => s.left = false,
            "ArrowRight" | "KeyD" => s.right = false,
            "ArrowUp" | "KeyW" => s.up = false,
            "ArrowDown" | "KeyS" => s.down = false,
            "Space" | "ShiftLeft" | "ShiftRight" => s.drift_key = false,
            "KeyN" | "ControlLeft" | "ControlRight" | "KeyX" => s.nitro_key = false,
            _ => {}
        }
    });
    let _ = w.add_event_listener_with_callback("keydown", down.as_ref().unchecked_ref());
    let _ = w.add_event_listener_with_callback("keyup", up.as_ref().unchecked_ref());
    down.forget();
    up.forget();
}

/// Finger (or mouse) held on the canvas steers by horizontal position relative to the centre.
fn install_pointer(canvas: &HtmlCanvasElement, state: Rc<RefCell<InputState>>) {
    let steer_at = |c: &HtmlCanvasElement, x: i32| -> f32 {
        let w = c.client_width().max(1) as f32;
        ((x as f32 / w) * 2.0 - 1.0).clamp(-1.0, 1.0) * 1.5
    };
    let c = canvas.clone();
    let st = state.clone();
    let pdown = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
        let mut s = st.borrow_mut();
        s.any_input = true;
        if e.pointer_type() == "touch" {
            s.touch_mode = true;
            s.last_device = "touch";
            if let Some(t) = crate::web::el("touch") {
                let _ = t.class_list().add_1("on");
            }
        } else {
            s.last_device = "mouse";
        }
        if s.steer_pointer.is_none() {
            s.steer_pointer = Some(e.pointer_id());
            s.touch_steer = steer_at(&c, e.client_x()).clamp(-1.0, 1.0);
            let _ = c.set_pointer_capture(e.pointer_id());
        }
    });
    let c2 = canvas.clone();
    let st = state.clone();
    let pmove = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
        let mut s = st.borrow_mut();
        if s.steer_pointer == Some(e.pointer_id()) {
            s.touch_steer = steer_at(&c2, e.client_x()).clamp(-1.0, 1.0);
        }
    });
    let pup = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
        let mut s = state.borrow_mut();
        if s.steer_pointer == Some(e.pointer_id()) {
            s.steer_pointer = None;
            s.touch_steer = 0.0;
        }
    });
    let _ = canvas.add_event_listener_with_callback("pointerdown", pdown.as_ref().unchecked_ref());
    let _ = canvas.add_event_listener_with_callback("pointermove", pmove.as_ref().unchecked_ref());
    let _ = canvas.add_event_listener_with_callback("pointerup", pup.as_ref().unchecked_ref());
    let _ = canvas.add_event_listener_with_callback("pointercancel", pup.as_ref().unchecked_ref());
    pdown.forget();
    pmove.forget();
    pup.forget();
}

/// On-screen BRAKE / DRIFT / NITRO buttons (shown once a touch is seen).
fn install_touch_buttons(state: Rc<RefCell<InputState>>) {
    for (id, which) in [("tb-brake", 0u8), ("tb-drift", 1), ("tb-nitro", 2)] {
        let Some(btn) = crate::web::el(id) else { continue };
        let set = |s: &mut InputState, which: u8, on: bool| match which {
            0 => s.touch_brake = on,
            1 => s.touch_drift = on,
            _ => s.touch_nitro = on,
        };
        let st = state.clone();
        let b = btn.clone();
        let down = Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
            e.prevent_default();
            let mut s = st.borrow_mut();
            s.any_input = true;
            s.touch_mode = true;
            set(&mut s, which, true);
            let _ = b.set_pointer_capture(e.pointer_id());
            let _ = b.class_list().add_1("held");
        });
        let st = state.clone();
        let b: HtmlElement = btn.clone();
        let up = Closure::<dyn FnMut(PointerEvent)>::new(move |_e: PointerEvent| {
            set(&mut st.borrow_mut(), which, false);
            let _ = b.class_list().remove_1("held");
        });
        let _ = btn.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref());
        let _ = btn.add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref());
        let _ = btn.add_event_listener_with_callback("pointercancel", up.as_ref().unchecked_ref());
        down.forget();
        up.forget();
    }
}
