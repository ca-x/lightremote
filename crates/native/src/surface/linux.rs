use gtk::{gdk, glib, prelude::*};
use nsh_platform_linux::{ComponentContext, Components, NativeComponent, gtk4 as gtk};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

pub fn components() -> Components {
    let components = Components::default();
    components.register(
        super::NAME,
        Rc::new(|context| Box::new(Surface::new(context))),
    );
    components
}
struct Surface {
    picture: gtk::Picture,
    context: ComponentContext,
}
impl Surface {
    fn new(context: ComponentContext) -> Self {
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.set_focusable(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        let motion = gtk::EventControllerMotion::new();
        let output = context.clone();
        motion.connect_motion(move |controller, x, y| {
            if let Some(widget) = controller.widget().and_downcast::<gtk::Picture>() {
                output.emit(
                    "input",
                    position("move", x, y, &widget, controller.current_event_state()),
                );
            }
        });
        picture.add_controller(motion);
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let output = context.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(widget) = gesture.widget().and_downcast::<gtk::Picture>() else {
                return;
            };
            widget.grab_focus();
            if let Some(button) = button(gesture.current_button()) {
                let mut event = position("button", x, y, &widget, gesture.current_event_state());
                event["button"] = button.into();
                event["down"] = true.into();
                output.emit("input", event);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        let output = context.clone();
        click.connect_released(move |gesture, _, x, y| {
            let Some(widget) = gesture.widget().and_downcast::<gtk::Picture>() else {
                return;
            };
            if let Some(button) = button(gesture.current_button()) {
                let mut event = position("button", x, y, &widget, gesture.current_event_state());
                event["button"] = button.into();
                event["down"] = false.into();
                output.emit("input", event);
            }
        });
        let output = context.clone();
        click.connect_cancel(move |_, _| output.emit("input", json!({"type":"release"})));
        picture.add_controller(click);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let output = context.clone();
        scroll.connect_scroll(move |controller, dx, dy| {
            if !controller.widget().is_some_and(|widget| widget.has_focus()) {
                return glib::Propagation::Proceed;
            }
            output.emit("input", json!({"type":"scroll","dx":dx,"dy":dy}));
            glib::Propagation::Stop
        });
        picture.add_controller(scroll);
        let keys = gtk::EventControllerKey::new();
        let held = Rc::new(RefCell::new(BTreeMap::<u32, String>::new()));
        let output = context.clone();
        let down = held.clone();
        keys.connect_key_pressed(move |_, key, code, _| {
            if let Some(name) = key.name().as_deref().and_then(super::key_name).or_else(|| {
                key.to_unicode()
                    .and_then(|ch| super::key_name(&ch.to_string()))
            }) {
                if down.borrow_mut().insert(code, name.clone()).is_none() {
                    output.emit("input", json!({"type":"key","key":name,"down":true}));
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        let output = context.clone();
        let up = held.clone();
        keys.connect_key_released(move |_, _, code, _| {
            if let Some(key) = up.borrow_mut().remove(&code) {
                output.emit("input", json!({"type":"key","key":key,"down":false}));
            }
        });
        picture.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let output = context.clone();
        focus.connect_enter(move |_| output.emit("input", json!({"type":"focus"})));
        let output = context.clone();
        let focus_keys = held.clone();
        focus.connect_leave(move |_| {
            focus_keys.borrow_mut().clear();
            output.emit("input", json!({"type":"release"}));
        });
        picture.add_controller(focus);
        let output = context.clone();
        picture.connect_root_notify(move |widget| {
            if let Some(window) = widget.root().and_downcast::<gtk::Window>() {
                let output = output.clone();
                let weak = widget.downgrade();
                let held = held.clone();
                window.connect_is_active_notify(move |window| {
                    if !window.is_active() && weak.upgrade().is_some() {
                        held.borrow_mut().clear();
                        output.emit("input", json!({"type":"release"}));
                    }
                });
            }
        });
        Self { picture, context }
    }
}
fn button(number: u32) -> Option<&'static str> {
    match number {
        1 => Some("left"),
        2 => Some("middle"),
        3 => Some("right"),
        _ => None,
    }
}
fn position(kind: &str, x: f64, y: f64, widget: &gtk::Picture, state: gdk::ModifierType) -> Value {
    let modifiers: Vec<_> = [
        (gdk::ModifierType::SHIFT_MASK, "Shift"),
        (gdk::ModifierType::CONTROL_MASK, "Control"),
        (gdk::ModifierType::ALT_MASK, "Alt"),
        (gdk::ModifierType::SUPER_MASK, "Meta"),
    ]
    .into_iter()
    .filter_map(|(flag, name)| state.contains(flag).then_some(name))
    .collect();
    json!({"type":kind,"x":x,"y":y,"width":widget.width(),"height":widget.height(),"modifiers":modifiers})
}
impl NativeComponent for Surface {
    fn widget(&self) -> gtk::Widget {
        self.picture.clone().upcast()
    }
    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        if name != "frame" {
            return;
        }
        let uri = value.and_then(Value::as_str).unwrap_or_default();
        if uri.is_empty() {
            self.picture.set_paintable(None::<&gdk::Paintable>);
            return;
        }
        match nsh_core::image::decode_data_uri(uri)
            .map_err(|error| error.to_string())
            .and_then(|(_, bytes)| {
                crate::desktop::admit_jpeg(&bytes).map_err(|error| error.to_string())?;
                gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))
                    .map_err(|error| error.to_string())
            }) {
            Ok(texture) => self.picture.set_paintable(Some(&texture)),
            Err(error) => self.context.emit(
                "input",
                json!({"type":"error","message":format!("Cannot display JPEG: {error}")}),
            ),
        }
    }
}
