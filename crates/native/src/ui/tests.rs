use super::*;
use nsh_engine::{ColorScheme, EngineConfig, Environment, Size};
use nsh_testing::Headless;

fn start<C: Component<Props = ()>>(dark: bool, reduce_motion: bool) -> Headless {
    let root = nsh_core::element::Root::of::<C>()
        .tokens(nsh_base::tokens_of(Some(include_str!("theme.json"))).unwrap());
    Headless::start(EngineConfig {
        viewport: Size::new(px(760.0), px(700.0)),
        environment: Environment {
            os: nsh_core::platform::Os::Linux,
            idiom: nsh_core::platform::Idiom::Desktop,
            color_scheme: if dark {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            },
            reduce_motion,
            ..Environment::default()
        },
        ..EngineConfig::new(root)
    })
    .unwrap()
}

#[component]
fn LimitedSession() -> impl IntoElement {
    let (client, _updates) = Client::start().unwrap();
    workspace(client, signal(State {
        connected: true,
        capabilities: json!({"desktop":true,"input":false,"shell":false,"file_read":true,"file_write":false,"clipboard":false}).to_string(),
        displays: json!({"displays":[{"id":0,"name":"Main","width":1920,"height":1080},{"id":1,"name":"Side","width":1280,"height":720}]}).to_string(),
        status: "Authenticated".into(),
        ..State::default()
    }))
}

#[component]
fn ConnectingSession() -> impl IntoElement {
    let (client, _updates) = Client::start().unwrap();
    workspace(
        client,
        signal(State {
            connecting: true,
            status: "Connecting and authenticating…".into(),
            ..State::default()
        }),
    )
}

#[component]
fn NoDesktopSession() -> impl IntoElement {
    let (client, _updates) = Client::start().unwrap();
    workspace(
        client,
        signal(State {
            connected: true,
            capabilities: json!({"desktop": false}).to_string(),
            ..State::default()
        }),
    )
}

#[component]
fn CapturedSession() -> impl IntoElement {
    let (client, _updates) = Client::start().unwrap();
    workspace(
        client,
        signal(State {
            connected: true,
            capabilities: json!({"desktop": true}).to_string(),
            screenshot: b"captured frame".as_slice().into(),
            ..State::default()
        }),
    )
}

#[test]
fn denied_desktop_explains_permission_and_offers_session_details() -> Result<(), String> {
    let mut app = start::<NoDesktopSession>(false, true);
    app.wait_for("Desktop viewing isn’t granted by this host.")?;
    assert!(
        !app.screen()
            .shows("Choose a display, then start viewing. No capture starts automatically.")
    );
    assert!(!app.screen().enabled("role:button[Start viewing]")?);
    app.press("role:button[Session permissions]")?;
    app.wait_for("The host decides what this session can do. Everything defaults off.")?;
    Ok(())
}

#[test]
fn desktop_tools_escape_and_cancelled_save_do_not_write() -> Result<(), String> {
    let mut app = start::<CapturedSession>(true, true);
    app.wait_for("Connected")?;
    assert!(!app.screen().matches("role:button[Save image…]")?);
    app.press("label:Desktop tools")?;
    app.wait_for("Save captured image")?;
    app.press_key("Escape")?;
    assert!(!app.screen().matches("role:button[Save image…]")?);
    app.press("label:Desktop tools")?;
    app.on_pick_save_path(None)?;
    app.press("role:button[Save image…]")?;
    app.wait(std::time::Duration::from_millis(50))?;
    let requests = app.screen().file_requests();
    assert_eq!(requests.len(), 1);
    assert!(
        matches!(&requests[0], nsh_core::file_dialog::FileRequest::SaveDialog(dialog) if dialog.file_name.as_deref() == Some("remote-desktop.jpg"))
    );
    assert!(!app.screen().matches("role:alert")?);
    Ok(())
}

#[test]
fn short_connection_window_keeps_expanded_help_in_a_scroll_region() -> Result<(), String> {
    let mut app = start::<App>(false, true);
    app.set_viewport(Size::new(px(640.0), px(320.0)))?;
    app.scroll("label:Connection form", px(240.0))?;
    app.press("role:button[Connection help]")?;
    app.wait_for("Invitations work once. The host chooses which tools you may use; viewing and control never start automatically.")?;
    assert!(app.screen().matches("label:Invitation")?);
    assert!(app.screen().matches("role:button[Connect]")?);
    Ok(())
}

#[test]
fn disconnected_window_is_a_connection_task_not_a_disabled_dashboard() -> Result<(), String> {
    for dark in [false, true] {
        let mut app = start::<App>(dark, true);
        app.wait_for("Not connected")?;
        assert!(app.screen().shows("roc-remote"));
        assert!(!app.screen().enabled("role:button[Connect]")?);
        app.wait_for("Connect to a host")?;
        assert!(!app.screen().matches("role:tab[Desktop]")?);
        assert!(!app.screen().matches("role:button[Start viewing]")?);
        assert!(!app.screen().matches("role:button[Disconnect]")?);
        assert!(
            !app.screen()
                .shows("Your remote workspace. One private session.")
        );
    }
    Ok(())
}

#[test]
fn drafts_survive_workspace_switches() -> Result<(), String> {
    let mut app = start::<LimitedSession>(false, false);
    app.wait_for("Connected")?;
    app.press("role:tab[Clipboard]")?;
    app.type_text("label:Clipboard editor", "a local draft")?;
    app.press("role:tab[Files]")?;
    app.type_text("label:Local file path", "/tmp/new-file")?;
    app.press("role:tab[Clipboard]")?;
    assert_eq!(
        app.screen().input_text("label:Clipboard editor")?,
        Some("a local draft")
    );
    app.press("role:tab[Files]")?;
    assert_eq!(
        app.screen().input_text("label:Local file path")?,
        Some("/tmp/new-file")
    );
    Ok(())
}

#[test]
fn keyboard_arrows_change_the_workspace() -> Result<(), String> {
    let mut app = start::<LimitedSession>(false, false);
    app.wait_for("Connected")?;
    app.press_key("tab")?;
    app.press_key("ArrowRight")?;
    app.wait_for("Remote terminal")?;
    assert!(app.screen().selected("role:tab[Terminal]")?);
    app.press_key("End")?;
    app.wait_for("Session permissions")?;
    assert!(app.screen().selected("role:tab[Session]")?);
    Ok(())
}

#[test]
fn invalid_invitation_reports_failure_and_clears_secret_field() -> Result<(), String> {
    let mut app = start::<App>(false, true);
    app.wait_for("Not connected")?;
    app.type_text("label:Invitation", "not-an-invitation")?;
    app.press("role:button[Connect]")?;
    app.wait_for("Invalid invitation. Paste the complete invitation URL or fragment.")?;
    assert_eq!(app.screen().input_text("label:Invitation")?, Some(""));
    assert!(!app.screen().matches("role:button[Disconnect]")?);
    assert!(app.screen().matches("role:alert")?);
    app.press("role:button[Dismiss]")?;
    assert!(
        !app.screen()
            .shows("Invalid invitation. Paste the complete invitation URL or fragment.")
    );
    assert!(!app.screen().matches("role:alert")?);
    Ok(())
}

#[test]
fn host_permissions_control_read_and_write_independently() -> Result<(), String> {
    let mut app = start::<LimitedSession>(true, true);
    app.wait_for("Connected")?;
    assert_eq!(
        app.screen()
            .accessibility_level("role:header[Remote desktop]")?,
        Some(2)
    );
    assert!(app.screen().enabled("role:button[Start viewing]")?);
    assert!(!app.screen().enabled("role:button[Release input]")?);
    app.press("role:tab[Terminal]")?;
    assert!(!app.screen().enabled("role:button[Open shell]")?);
    app.press("role:tab[Files]")?;
    app.type_text("label:Local file path", "/tmp/new-file")?;
    assert!(app.screen().enabled("role:button[Download file]")?);
    assert!(!app.screen().enabled("role:button[Upload file]")?);
    app.press("role:tab[Clipboard]")?;
    assert!(!app.screen().enabled("role:button[Write remote]")?);
    Ok(())
}

#[test]
fn connecting_can_be_cancelled_but_not_resubmitted() -> Result<(), String> {
    let mut app = start::<ConnectingSession>(false, false);
    app.wait_for("Connecting…")?;
    assert!(!app.screen().enabled("role:button[Connect]")?);
    assert!(!app.screen().enabled("label:Invitation")?);
    assert!(app.screen().enabled("role:button[Cancel]")?);
    Ok(())
}

#[test]
fn capabilities_fail_closed() {
    let mut state = State {
        capabilities: json!({"desktop":true}).to_string(),
        ..State::default()
    };
    assert!(!state.allows("desktop"));
    state.connected = true;
    assert!(state.allows("desktop"));
    assert!(!state.allows("input"));
    for invalid in ["", "not JSON", "[]", "{\"desktop\":\"true\"}"] {
        state.capabilities = invalid.to_owned();
        assert!(!state.allows("desktop"));
    }
}

#[test]
fn display_picker_uses_host_labels_and_preserves_selection() -> Result<(), String> {
    let mut app = start::<LimitedSession>(false, true);
    app.wait_for("Connected")?;
    app.press("label:Display")?;
    app.wait_for("Display 1 · Side · 1280×720")?;
    app.press("Display 1 · Side · 1280×720")?;
    app.press("role:tab[Session]")?;
    app.press("role:tab[Desktop]")?;
    app.wait_for("Display 1 · Side · 1280×720")?;
    Ok(())
}

#[component]
fn DirectoryPreview() -> impl IntoElement {
    file_listing(signal(State {
        listing: json!({"entries":(0..200).map(|index| json!({"name":format!("file-{index:03}"),"type":"file","size":index})).collect::<Vec<_>>()} ).to_string(),
        ..State::default()
    }))
}

#[test]
fn directory_cells_follow_recycled_indices() -> Result<(), String> {
    let mut app = start::<DirectoryPreview>(true, true);
    app.wait_for("file-000")?;
    assert!(
        app.screen()
            .texts()
            .iter()
            .filter(|text| text.starts_with("file-"))
            .count()
            < 200
    );
    app.scroll("id:directory-list", px(2400.0))?;
    assert!(app.screen().texts().iter().any(|text| {
        text.strip_prefix("file-")
            .and_then(|index| index.parse::<u32>().ok())
            .is_some_and(|index| index >= 50)
    }));
    app.scroll("id:directory-list", px(-2400.0))?;
    app.wait_for("file-000")?;
    Ok(())
}

#[component]
fn TerminalGeometry() -> impl IntoElement {
    let selected = signal(Workspace::Terminal);
    let size = signal((0_u32, 0_u32));
    view()
        .flex_1()
        .child(action(
            "Hide terminal",
            Action::Secondary,
            || false,
            move || selected.set(Workspace::Files),
        ))
        .child(action(
            "Show terminal",
            Action::Secondary,
            || false,
            move || selected.set(Workspace::Terminal),
        ))
        .child(text(move || {
            size.with(|(cols, rows)| format!("Terminal geometry: {cols}×{rows}"))
        }))
        .child(view().relative().flex_1().min_h_0().child(
            section(selected, Workspace::Terminal).child(terminal().flex_1().on_resize(
                move |event: &TerminalEvent| {
                    if let TerminalEvent::Resize(cols, rows) = event {
                        size.set((*cols, *rows));
                    }
                },
            )),
        ))
}

#[test]
fn switching_tools_preserves_terminal_geometry() -> Result<(), String> {
    let mut app = start::<TerminalGeometry>(false, true);
    app.wait_for("Hide terminal")?;
    let before = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.starts_with("Terminal geometry:"))
        .unwrap()
        .to_owned();
    assert_ne!(before, "Terminal geometry: 0×0");
    assert_ne!(before, "Terminal geometry: 1×1");
    app.press("Hide terminal")?;
    assert!(
        app.shows(&before),
        "Hidden terminal must retain its geometry; {:?}",
        app.screen().texts()
    );
    app.press("Show terminal")?;
    assert!(app.shows(&before));
    Ok(())
}

#[component]
fn BusySession() -> impl IntoElement {
    let (client, _updates) = Client::start().unwrap();
    workspace(
        client,
        signal(State {
            connected: true,
            capabilities: json!({"file_read":true,"file_write":true,"clipboard":true}).to_string(),
            pending: [
                Operation::Listing,
                Operation::Transfer,
                Operation::Clipboard,
            ]
            .into_iter()
            .collect(),
            listing_path: "previous-directory".into(),
            transfer_status: "Transfer in progress…".into(),
            clipboard_status: "Clipboard request in progress…".into(),
            ..State::default()
        }),
    )
}

#[test]
fn pending_actions_disable_duplicates_and_preserve_confirmed_directory() -> Result<(), String> {
    let mut app = start::<BusySession>(true, true);
    app.wait_for("Connected")?;
    assert!(!app.screen().shows("Invitation"));
    app.press("role:tab[Files]")?;
    app.type_text("label:Local file path", "/tmp/transfer")?;
    assert!(!app.screen().enabled("role:button[List directory]")?);
    assert!(!app.screen().enabled("role:button[Download file]")?);
    assert!(!app.screen().enabled("role:button[Upload file]")?);
    app.wait_for("Loading directory… Showing: previous-directory")?;
    app.press("role:tab[Clipboard]")?;
    assert!(!app.screen().enabled("role:button[Read remote]")?);
    assert!(!app.screen().enabled("role:button[Write remote]")?);
    app.wait_for("Clipboard request in progress…")?;
    Ok(())
}

#[component]
fn MobilePreview() -> impl IntoElement {
    let (client, _updates) = Client::start().unwrap();
    mobile::workspace(client, signal(State::default()))
}

#[test]
fn mobile_preview_is_explicit_read_only_and_keeps_connection_reachable() -> Result<(), String> {
    for dark in [false, true] {
        let mut app = start::<MobilePreview>(dark, true);
        app.set_viewport(Size::new(px(360.0), px(780.0)))?;
        app.set_environment(
            "Android safe area, large text and software keyboard",
            |environment| {
                environment.os = nsh_core::platform::Os::Android;
                environment.idiom = nsh_core::platform::Idiom::Phone;
                environment.font_scale = 1.3;
                environment.safe_area.top = px(24.0);
                environment.safe_area.bottom = px(16.0);
                environment.keyboard_height = px(240.0);
            },
        )?;
        app.wait_for("Android preview")?;
        assert!(app.screen().shows("Experimental viewer. Touch control, clipboard, terminal and local file transfers are not available in this build."));
        assert!(!app.screen().enabled("role:button[Connect]")?);
        assert!(!app.screen().shows("Upload file"));
        assert!(!app.screen().shows("Open shell"));
        app.press("role:button[Viewing help]")?;
        app.wait_for("This preview shows host display 0. Capture starts only when you ask. Stop viewing ends the stream; Disconnect ends the session.")?;
        app.press("role:button[Viewing help]")?;
        app.type_text("label:Invitation", "not-an-invitation")?;
        assert!(app.screen().enabled("role:button[Connect]")?);
    }
    Ok(())
}

#[component]
fn HelpExample() -> impl IntoElement {
    help("Viewing help", "Choose a display, then start viewing.")
}

#[test]
fn help_disclosure_opens_with_keyboard_and_closes_with_a_second_press() -> Result<(), String> {
    for dark in [false, true] {
        let mut app = start::<HelpExample>(dark, true);
        app.wait_for("Viewing help")?;
        assert!(!app.screen().shows("Choose a display, then start viewing."));
        app.press_key("tab")?;
        app.press_key("Enter")?;
        assert_eq!(
            app.screen()
                .accessibility_state("role:button[Viewing help]")?
                .expanded,
            Some(true)
        );
        app.wait_for("Choose a display, then start viewing.")?;
        app.press_key("Space")?;
        assert_eq!(
            app.screen()
                .accessibility_state("role:button[Viewing help]")?
                .expanded,
            Some(false)
        );
        app.press_key("Escape")?;
        assert!(!app.screen().shows("Choose a display, then start viewing."));
    }
    Ok(())
}
