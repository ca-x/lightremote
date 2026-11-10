use super::*;
use nsh_base::{nav_page, nav_stack};

pub(super) fn workspace(client: Client, snapshot: Signal<State>) -> AnyElement {
    let invitation = signal(String::new());
    let connect = client.clone();
    let disconnect = client.clone();
    let refresh = client.clone();
    let capture = client.clone();
    let stop = client.clone();
    let dismiss = client;
    let content = view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .pt(move || safe_area().top)
        .pb(move || safe_area().bottom.max(keyboard_height()))
        .pl(move || safe_area().left)
        .pr(move || safe_area().right)
        .child(scroll().flex_1().child(view().p_4().gap_4()
            .child(heading("Android preview"))
            .child(hint("Experimental viewer. Touch control, clipboard, terminal and local file transfers are not available in this build."))
            .child(view().gap_2().style_when(move || snapshot.with(|state| state.connected), |style| style.hidden())
                .child(hint("Invitation"))
                .child(field(invitation, "Invitation", "Paste invitation URL or fragment").masked(true)
                    .disabled(move || snapshot.with(|state| state.connecting)))
                .child(action("Connect", Action::Primary,
                    move || invitation.with(|value| value.trim().is_empty()) || snapshot.with(|state| state.connecting),
                    move || {
                        connect.send(Command::Connect(invitation.get_untracked().trim().to_owned()));
                        invitation.set(String::new());
                    })))
            .child(text(move || snapshot.with(|state| state.status.clone())).text_sm())
            .child(action(move || snapshot.with(|state| if state.connecting { "Cancel" } else { "Disconnect" }), Action::Secondary,
                move || snapshot.with(|state| !state.connected && !state.connecting),
                move || disconnect.send(Command::Disconnect)))
            .child(view().gap_2().style_when(move || snapshot.with(|state| state.error.is_empty()), |style| style.hidden())
                .child(text(move || snapshot.with(|state| state.error.clone())).role(AccessibilityRole::Alert)
                    .text_sm().text_color(move || theme().destructive))
                .child(action("Dismiss", Action::Secondary, || false, move || dismiss.dismiss_error())))
            .child(heading("Remote desktop"))
            .child(permission_notice(snapshot, "desktop"))
            .child(hint("Display 0 · Read-only · Capture starts only when you ask."))
            .child(help("Viewing help", "This preview shows host display 0. Capture starts only when you ask. Stop viewing ends the stream; Disconnect ends the session."))
            .child(row()
                .child(action("Start viewing", Action::Primary,
                    move || snapshot.with(|state| !state.allows("desktop") || state.desktop_running),
                    move || capture.send(Command::Desktop { display: 0, single: false })))
                .child(action("Stop viewing", Action::Secondary,
                    move || !snapshot.with(|state| state.desktop_running),
                    move || stop.send(Command::StopDesktop))))
            .child(image(move || snapshot.with(|state| state.desktop.to_string()))
                .label("Read-only remote desktop preview")
                .w_full().h(move || window().height * 0.4)
                .bg(move || theme().color("remote-canvas").expect("remote canvas token"))
                .style_when(move || snapshot.with(|state| state.desktop.is_empty()), |style| style.hidden()))
            .child(heading("Host permissions"))
            .child(permissions(snapshot))
            .child(text(move || snapshot.with(State::platform_guidance)).text_sm())
            .child(action("Refresh permissions", Action::Secondary,
                move || !snapshot.with(|state| state.connected),
                move || refresh.send(Command::Capabilities)))
            .child(icon_credits())
        ));
    nav_stack()
        .flex_1()
        .child(
            nav_page()
                .title("roc-remote")
                .header(view().p_4().child(heading("roc-remote")))
                .child(content),
        )
        .into_element()
}
