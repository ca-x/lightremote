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

#[test]
fn disconnected_tools_are_disabled_in_light_and_dark() -> Result<(), String> {
    for dark in [false, true] {
        let mut app = start::<App>(dark, true);
        app.wait_for("Not connected")?;
        assert!(!app.screen().enabled("role:button[Connect]")?);
        assert!(!app.screen().enabled("role:button[Start viewing]")?);
        assert!(!app.screen().enabled("role:button[Capture once]")?);
        app.press("role:tab[Terminal]")?;
        assert!(!app.screen().enabled("role:button[Open shell]")?);
        app.press("role:tab[Files]")?;
        assert!(!app.screen().enabled("role:button[List directory]")?);
        app.press("role:tab[Clipboard]")?;
        assert!(!app.screen().enabled("role:button[Read remote]")?);
        assert!(!app.screen().enabled("role:button[Write remote]")?);
    }
    Ok(())
}

#[test]
fn drafts_survive_workspace_switches() -> Result<(), String> {
    let mut app = start::<App>(false, false);
    app.wait_for("Not connected")?;
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
    let mut app = start::<App>(false, false);
    app.wait_for("Not connected")?;
    app.press_key("tab")?;
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
    assert!(!app.screen().enabled("role:button[Disconnect]")?);
    Ok(())
}

#[test]
fn host_permissions_control_read_and_write_independently() -> Result<(), String> {
    let mut app = start::<LimitedSession>(true, true);
    app.wait_for("Connected")?;
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
