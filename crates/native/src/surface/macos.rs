use nsh_platform_macos::{
    ComponentContext, Components, NativeComponent,
    objc2::{
        AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
        rc::Retained, sel,
    },
    objc2_app_kit::{
        NSEvent, NSEventModifierFlags, NSImage, NSImageScaling, NSImageView, NSTrackingArea,
        NSTrackingAreaOptions, NSView, NSWindowDidResignKeyNotification,
    },
    objc2_foundation::{NSData, NSNotification, NSNotificationCenter, NSObjectProtocol, NSRect},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

pub fn components() -> Components {
    let components = Components::default();
    components.register(
        super::NAME,
        Rc::new(|context| Box::new(Surface::new(context))),
    );
    components
}
struct Ivars {
    context: ComponentContext,
    keys: RefCell<BTreeMap<u16, String>>,
    modifiers: Cell<u64>,
}
define_class!(
    #[unsafe(super(NSImageView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    #[name = "roc-remoteDesktopInputView"]
    struct DesktopView;
    unsafe impl NSObjectProtocol for DesktopView {}
    impl DesktopView {
        #[unsafe(method(isFlipped))]
        fn flipped(&self)->bool { true }
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self)->bool { true }
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn movable(&self)->bool { false }
        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self)->bool {
            self.ivars().context.emit("input",json!({"type":"focus"}));true
        }
        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self)->bool { self.release();true }
        #[unsafe(method(windowInactive:))]
        fn inactive(&self,_note:&NSNotification) { self.release(); }
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self,event:&NSEvent) { self.ivars().context.emit("input",self.position("move",event)); }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self,event:&NSEvent) { self.move_pointer(event); }
        #[unsafe(method(rightMouseDragged:))]
        fn right_dragged(&self,event:&NSEvent) { self.move_pointer(event); }
        #[unsafe(method(otherMouseDragged:))]
        fn other_dragged(&self,event:&NSEvent) { self.move_pointer(event); }
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self,event:&NSEvent) { self.button(event,"left",true); }
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self,event:&NSEvent) { self.button(event,"left",false); }
        #[unsafe(method(rightMouseDown:))]
        fn right_down(&self,event:&NSEvent) { self.button(event,"right",true); }
        #[unsafe(method(rightMouseUp:))]
        fn right_up(&self,event:&NSEvent) { self.button(event,"right",false); }
        #[unsafe(method(otherMouseDown:))]
        fn other_down(&self,event:&NSEvent) { if event.buttonNumber()==2 {self.button(event,"middle",true);} }
        #[unsafe(method(otherMouseUp:))]
        fn other_up(&self,event:&NSEvent) { if event.buttonNumber()==2 {self.button(event,"middle",false);} }
        #[unsafe(method(scrollWheel:))]
        fn wheel(&self,event:&NSEvent) {
            // Precise touchpads report points; accumulate fractions of a notch in Rust UI.
            let divisor=if event.hasPreciseScrollingDeltas(){40.0}else{1.0};
            self.ivars().context.emit("input",json!({"type":"scroll","dx":-event.scrollingDeltaX()/divisor,"dy":-event.scrollingDeltaY()/divisor}));
        }
        #[unsafe(method(keyDown:))]
        fn key_down(&self,event:&NSEvent) {
            let code=event.keyCode();
            if self.ivars().keys.borrow().contains_key(&code){return;}
            if let Some(key)=key(event) {
                self.ivars().keys.borrow_mut().insert(code,key.clone());
                self.ivars().context.emit("input",json!({"type":"key","key":key,"down":true}));
            }
        }
        #[unsafe(method(keyUp:))]
        fn key_up(&self,event:&NSEvent) {
            if let Some(key)=self.ivars().keys.borrow_mut().remove(&event.keyCode()) { self.ivars().context.emit("input",json!({"type":"key","key":key,"down":false})); }
        }
        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self,event:&NSEvent) {
            let flags=event.modifierFlags();
            let previous=self.ivars().modifiers.replace(flags.0 as u64);
            for (flag,key) in [(NSEventModifierFlags::Shift,"Shift"),(NSEventModifierFlags::Control,"Control"),(NSEventModifierFlags::Option,"Alt"),(NSEventModifierFlags::Command,"Meta")] {
                if (previous & flag.0 as u64 != 0)!=flags.contains(flag) {
                    self.ivars().context.emit("input",json!({"type":"key","key":key,"down":flags.contains(flag)}));
                }
            }
        }
    }
);
impl DesktopView {
    fn move_pointer(&self, event: &NSEvent) {
        self.ivars()
            .context
            .emit("input", self.position("move", event));
    }
    fn position(&self, kind: &str, event: &NSEvent) -> Value {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let size = self.bounds().size;
        let flags = event.modifierFlags();
        let modifiers: Vec<_> = [
            (NSEventModifierFlags::Shift, "Shift"),
            (NSEventModifierFlags::Control, "Control"),
            (NSEventModifierFlags::Option, "Alt"),
            (NSEventModifierFlags::Command, "Meta"),
        ]
        .into_iter()
        .filter_map(|(flag, name)| flags.contains(flag).then_some(name))
        .collect();
        json!({"type":kind,"x":point.x,"y":point.y,"width":size.width,"height":size.height,"modifiers":modifiers})
    }
    fn button(&self, event: &NSEvent, button: &str, down: bool) {
        if down && let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        let mut value = self.position("button", event);
        value["button"] = button.into();
        value["down"] = down.into();
        self.ivars().context.emit("input", value);
    }
    fn release(&self) {
        self.ivars().keys.borrow_mut().clear();
        self.ivars().modifiers.set(0);
        self.ivars()
            .context
            .emit("input", json!({"type":"release"}));
    }
}
fn key(event: &NSEvent) -> Option<String> {
    let name = match event.keyCode() {
        36 | 76 => "Enter",
        48 => "Tab",
        49 => "Space",
        51 => "Backspace",
        53 => "Escape",
        117 => "Delete",
        115 => "Home",
        119 => "End",
        116 => "PageUp",
        121 => "PageDown",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        122 => "F1",
        120 => "F2",
        99 => "F3",
        118 => "F4",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        100 => "F8",
        101 => "F9",
        109 => "F10",
        103 => "F11",
        111 => "F12",
        _ => {
            return event
                .charactersIgnoringModifiers()
                .and_then(|text| super::key_name(&text.to_string()));
        }
    };
    Some(name.into())
}
struct Surface {
    view: Retained<DesktopView>,
}
impl Surface {
    fn new(context: ComponentContext) -> Self {
        let mtm =
            MainThreadMarker::new().expect("NSH creates native components on the AppKit thread");
        let allocated = DesktopView::alloc(mtm).set_ivars(Ivars {
            context,
            keys: RefCell::default(),
            modifiers: Cell::new(0),
        });
        // SAFETY: Initialize this NSImageView subclass and install an owned tracking
        // area and selector observer. Surface removes the observer before view drop.
        let view: Retained<DesktopView> =
            unsafe { msg_send![super(allocated),initWithFrame:NSRect::ZERO] };
        view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        unsafe {
            let tracking = NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::ActiveInKeyWindow
                    | NSTrackingAreaOptions::InVisibleRect
                    | NSTrackingAreaOptions::EnabledDuringMouseDrag,
                Some(&view),
                None,
            );
            view.addTrackingArea(&tracking);
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &view,
                sel!(windowInactive:),
                Some(NSWindowDidResignKeyNotification),
                None,
            );
        }
        Self { view }
    }
}
impl NativeComponent for Surface {
    fn view(&self) -> Retained<NSView> {
        Retained::into_super(Retained::into_super(Retained::into_super(
            self.view.clone(),
        )))
    }
    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        if name != "frame" {
            return;
        }
        let uri = value.and_then(Value::as_str).unwrap_or_default();
        if uri.is_empty() {
            self.view.setImage(None);
            return;
        }
        let image = nsh_core::image::decode_data_uri(uri)
            .ok()
            .and_then(|(_, bytes)| {
                crate::desktop::admit_jpeg(&bytes).ok()?;
                NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&bytes))
            });
        match image {
            Some(image) => self.view.setImage(Some(&image)),
            None => self.view.ivars().context.emit(
                "input",
                json!({"type":"error","message":"Cannot decode desktop JPEG"}),
            ),
        }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        self.view.release();
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(&self.view);
        }
    }
}
