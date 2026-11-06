use super::*;

const ORIGIN: (u16, u16) = (1, 1);
const INNER: (u16, u16) = (11, 11);

/// A viewport-only `Areas`, origin `(1, 1)` and inner size `(11, 11)` —
/// a 13x13 outer rect at (0, 0), border excluded same as `viewport_inner`.
fn viewport() -> Areas {
    let rect = Rect {
        x: 0,
        y: 0,
        width: 13,
        height: 13,
    };
    Areas {
        rects: [(Panel::Viewport, rect); 5],
    }
}

#[test]
fn center_cell_maps_near_half() {
    let (x, y) = normalize(ORIGIN, INNER, 6, 6).unwrap();
    assert!((x - 0.5).abs() < 1e-6, "x={x}");
    assert!((y - 0.5).abs() < 1e-6, "y={y}");
}

#[test]
fn outside_viewport_reports_not_down_with_position_unchanged() {
    let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
    p.feed(MouseEventKind::Moved, 6, 6);
    let (first, _) = t.update(true, &v, &p);
    p.feed(MouseEventKind::Moved, 50, 50);
    let (second, down) = t.update(true, &v, &p);
    assert_eq!(first, second);
    assert_eq!(down, 0);
}

#[test]
fn press_starting_outside_and_dragging_inside_stays_not_down() {
    let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
    p.feed(MouseEventKind::Down(MouseButton::Left), 50, 50);
    let (_, down) = t.update(true, &v, &p);
    assert_eq!(down, 0);
    p.feed(MouseEventKind::Drag(MouseButton::Left), 6, 6);
    let (_, down) = t.update(true, &v, &p);
    assert_eq!(down, 0, "started outside — stays up even once inside");
}

#[test]
fn press_starting_inside_stays_down_while_dragging_outside() {
    let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
    p.feed(MouseEventKind::Down(MouseButton::Left), 6, 6);
    let (_, down) = t.update(true, &v, &p);
    assert_eq!(down, 1);
    p.feed(MouseEventKind::Drag(MouseButton::Left), 50, 50);
    let (pos, down) = t.update(true, &v, &p);
    assert_eq!(down, 1, "started inside — stays down even once outside");
    let (cx, cy) = normalize(ORIGIN, INNER, 6, 6).unwrap();
    assert_eq!(
        pos,
        [cx, cy],
        "position holds at the last cell inside the viewport"
    );
}

#[test]
fn edit_mode_never_reports_down() {
    let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
    p.feed(MouseEventKind::Down(MouseButton::Left), 6, 6);
    let (_, down) = t.update(false, &v, &p);
    assert_eq!(down, 0);
}

#[test]
fn release_requires_a_fresh_press_to_go_down_again() {
    let (mut p, mut t, v) = (Pointer::new(), Touch::new(), viewport());
    p.feed(MouseEventKind::Down(MouseButton::Left), 6, 6);
    t.update(true, &v, &p);
    p.feed(MouseEventKind::Up(MouseButton::Left), 6, 6);
    let (_, down) = t.update(true, &v, &p);
    assert_eq!(down, 0);
}

#[test]
fn parse_touch_reads_down_drag_up_and_dash() {
    let events = parse_touch("-\ndown 5,5\ndrag 6,7\nup 6,7\n").unwrap();
    assert_eq!(
        events,
        vec![
            None,
            Some((MouseEventKind::Down(MouseButton::Left), 5, 5)),
            Some((MouseEventKind::Drag(MouseButton::Left), 6, 7)),
            Some((MouseEventKind::Up(MouseButton::Left), 6, 7)),
        ]
    );
}

#[test]
fn parse_touch_skips_comments_and_blank_lines() {
    let events = parse_touch("# a comment\n\ndown 1,1\n").unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn parse_touch_rejects_an_unknown_command() {
    assert!(parse_touch("poke 1,1\n").is_err());
}

#[test]
fn scripted_touch_held_inside_reports_down_and_repeats_the_last_frame() {
    let mut ts = ScriptedTouch {
        events: parse_touch("down 5,5\n").unwrap(),
        pointer: Pointer::new(),
        touch: Touch::new(),
        areas: synthetic_viewport((10, 10)),
    };
    let k0 = ts.poll(0);
    assert_eq!(k0.mouse_down, 1);
    assert_eq!(k0.down, 0, "a touch script never sets keyboard bits");
    let k5 = ts.poll(5);
    assert_eq!(
        k5.mouse_down, 1,
        "past the script's end, the last frame repeats"
    );
}

#[test]
fn scripted_touch_outside_the_viewport_never_reports_down() {
    let mut ts = ScriptedTouch {
        events: parse_touch("down 50,50\n").unwrap(),
        pointer: Pointer::new(),
        touch: Touch::new(),
        areas: synthetic_viewport((10, 10)),
    };
    assert_eq!(ts.poll(0).mouse_down, 0);
}
