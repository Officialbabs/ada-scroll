use scroll_layout::{Action, Config, Rect, Scroll, Strip};

// 1000 wide with gaps of 10: the view is 980, half of it 485.
const AREA: Rect = Rect::new(0, 0, 1000, 600);

fn config() -> Config {
    Config { gap: 10, ..Config::default() }
}

fn strip(windows: &[u32]) -> Strip<u32> {
    let mut strip = Strip::new(config(), AREA);
    for &w in windows {
        strip.add(w);
    }
    strip
}

fn rect(strip: &Strip<u32>, window: u32) -> Rect {
    strip.placements().into_iter().find(|p| p.window == window).unwrap().rect
}

fn order(strip: &Strip<u32>) -> Vec<u32> {
    strip.placements().into_iter().map(|p| p.window).collect()
}

#[test]
fn one_window_sits_in_the_middle() {
    let s = strip(&[1]);
    assert_eq!(rect(&s, 1), Rect::new(258, 10, 485, 580));
}

#[test]
fn two_halves_fill_the_screen() {
    let s = strip(&[1, 2]);
    assert_eq!(rect(&s, 1), Rect::new(10, 10, 485, 580));
    assert_eq!(rect(&s, 2), Rect::new(505, 10, 485, 580));
}

#[test]
fn thirds_fill_the_screen() {
    let mut s = strip(&[1, 2, 3]);
    for w in [1, 2, 3] {
        s.focus(&w);
        s.apply(Action::NextWidth); // from a half to two thirds,
        s.apply(Action::NextWidth); // then round to a third
    }
    assert_eq!(s.width_of(&1), Some(320));
    assert_eq!(rect(&s, 1).x, 10);
    assert_eq!(rect(&s, 3).right(), 990);
}

#[test]
fn a_new_window_opens_right_of_the_focused_one_and_is_shown() {
    let mut s = strip(&[1, 2]);
    s.focus(&1);
    s.add(3);
    assert_eq!(order(&s), vec![1, 3, 2]);
    assert_eq!(s.focused(), Some(&3));
    let r = rect(&s, 3);
    assert!(r.x >= 10 && r.right() <= 990, "{r:?}");
}

#[test]
fn edge_scrolling_moves_only_as_far_as_needed() {
    let mut s = strip(&[1, 2, 3]);
    // Three halves: the strip is 1475 wide, the third column at its end.
    assert_eq!(s.scroll(), 495.0);
    assert_eq!(rect(&s, 3).right(), 990);
    s.apply(Action::FocusLeft);
    assert_eq!(s.scroll(), 495.0, "the middle column is already in view");
    s.apply(Action::FocusLeft);
    assert_eq!(s.scroll(), 0.0);
    assert_eq!(rect(&s, 1).x, 10);
}

#[test]
fn center_scrolling_keeps_focus_in_the_middle_but_not_past_the_ends() {
    let mut s = Strip::new(Config { gap: 10, scroll: Scroll::Center, ..Config::default() }, AREA);
    for w in [1, 2, 3, 4] {
        s.add(w);
    }
    s.focus(&2);
    let r = rect(&s, 2);
    assert_eq!(r.x + r.w / 2, 500);
    s.focus(&1);
    assert_eq!(rect(&s, 1).x, 10);
    s.focus(&4);
    assert_eq!(rect(&s, 4).right(), 990);
}

#[test]
fn closing_the_focused_window_focuses_the_column_that_takes_its_place() {
    let mut s = strip(&[1, 2, 3]);
    s.focus(&2);
    s.remove(&2);
    assert_eq!(s.focused(), Some(&3));
    s.remove(&3);
    assert_eq!(s.focused(), Some(&1));
    s.remove(&1);
    assert_eq!(s.focused(), None);
    assert!(s.is_empty());
}

#[test]
fn closing_another_window_keeps_focus() {
    let mut s = strip(&[1, 2, 3]);
    s.focus(&3);
    s.remove(&1);
    assert_eq!(s.focused(), Some(&3));
}

#[test]
fn moving_left_joins_the_column_and_moving_right_leaves_it() {
    let mut s = strip(&[1, 2]);
    s.apply(Action::MoveLeft);
    assert_eq!(s.column_count(), 1);
    assert_eq!(s.find(&2), Some((0, 1)));
    assert_eq!(rect(&s, 1), Rect::new(258, 10, 485, 285));
    assert_eq!(rect(&s, 2), Rect::new(258, 305, 485, 285));
    assert_eq!(s.focused(), Some(&2));

    s.apply(Action::MoveRight);
    assert_eq!(s.column_count(), 2);
    assert_eq!(order(&s), vec![1, 2]);
    assert_eq!(s.focused(), Some(&2));
}

#[test]
fn moving_out_to_the_left_makes_a_column_on_the_left() {
    let mut s = strip(&[1, 2]);
    s.apply(Action::MoveLeft); // [1, 2] in one column
    s.apply(Action::MoveLeft); // 2 out to the left
    assert_eq!(order(&s), vec![2, 1]);
    assert_eq!(s.focused(), Some(&2));
}

#[test]
fn moving_at_the_ends_does_nothing() {
    let mut s = strip(&[1]);
    s.apply(Action::MoveLeft);
    s.apply(Action::MoveRight);
    s.apply(Action::MoveColumnLeft);
    assert_eq!(order(&s), vec![1]);
    assert_eq!(s.column_count(), 1);
}

#[test]
fn up_and_down_within_a_column() {
    let mut s = strip(&[1, 2, 3]);
    s.apply(Action::MoveLeft);
    s.focus(&1);
    s.apply(Action::MoveRight); // 1 joins the column of 2 and 3
    assert_eq!(order(&s), vec![2, 3, 1]);
    s.apply(Action::MoveUp);
    assert_eq!(order(&s), vec![2, 1, 3]);
    s.apply(Action::FocusUp);
    assert_eq!(s.focused(), Some(&2));
    s.apply(Action::FocusUp);
    assert_eq!(s.focused(), Some(&2));
    s.apply(Action::FocusDown);
    s.apply(Action::FocusDown);
    s.apply(Action::FocusDown);
    assert_eq!(s.focused(), Some(&3));
}

#[test]
fn columns_move_and_keep_focus() {
    let mut s = strip(&[1, 2, 3]);
    s.apply(Action::MoveColumnFirst);
    assert_eq!(order(&s), vec![3, 1, 2]);
    s.apply(Action::MoveColumnRight);
    assert_eq!(order(&s), vec![1, 3, 2]);
    s.apply(Action::MoveColumnLast);
    assert_eq!(order(&s), vec![1, 2, 3]);
    assert_eq!(s.focused(), Some(&3));
}

#[test]
fn widths_cycle_through_the_presets() {
    let mut s = strip(&[1]);
    assert_eq!(s.width_of(&1), Some(485));
    s.apply(Action::NextWidth);
    assert_eq!(s.width_of(&1), Some(650));
    s.apply(Action::NextWidth);
    assert_eq!(s.width_of(&1), Some(320));
    s.apply(Action::PreviousWidth);
    assert_eq!(s.width_of(&1), Some(650));
}

#[test]
fn full_width_toggles_back() {
    let mut s = strip(&[1, 2]);
    s.apply(Action::FullWidth);
    assert_eq!(s.width_of(&2), Some(980));
    assert_eq!(rect(&s, 2), Rect::new(10, 10, 980, 580));
    s.apply(Action::FullWidth);
    assert_eq!(s.width_of(&2), Some(485));
}

#[test]
fn wider_and_narrower_stay_within_the_screen() {
    let mut s = strip(&[1]);
    for _ in 0..20 {
        s.apply(Action::Wider);
    }
    assert_eq!(s.width_of(&1), Some(980));
    for _ in 0..40 {
        s.apply(Action::Narrower);
    }
    assert_eq!(s.width_of(&1), Some(1));
}

#[test]
fn a_column_is_never_narrower_than_its_windows_allow() {
    let mut s = strip(&[1]);
    s.set_min_width(&1, 600);
    assert_eq!(s.width_of(&1), Some(600));
    s.set_width(&1, 100);
    assert_eq!(s.width_of(&1), Some(600));
    s.set_min_width(&1, 5000);
    assert_eq!(s.width_of(&1), Some(980), "but never wider than the screen");
}

#[test]
fn equalize_fills_the_screen_with_the_columns_on_it() {
    let mut s = strip(&[1, 2, 3]);
    s.focus(&1);
    s.apply(Action::NextWidth); // 650 + 485 + 485: 2 and 3 not both on screen
    s.focus(&2);
    s.apply(Action::EqualizeVisible);
    let (a, b) = (rect(&s, 1), rect(&s, 2));
    assert_eq!((a.x, a.w), (10, 485));
    assert_eq!((b.x, b.right()), (505, 990));
    assert_eq!(s.focused(), Some(&2));
}

#[test]
fn stacked_columns_show_only_the_focused_window() {
    let mut s = strip(&[1, 2]);
    s.apply(Action::MoveLeft);
    s.apply(Action::ToggleStacked);
    let p = s.placements();
    assert_eq!(p[0].rect, Rect::new(258, 10, 485, 580));
    assert_eq!(p[1].rect, Rect::new(258, 10, 485, 580));
    assert!(!p[0].shown && p[1].shown && p[1].focused);
    s.apply(Action::FocusUp);
    let p = s.placements();
    assert!(p[0].shown && !p[1].shown);
}

#[test]
fn scrolling_by_hand_keeps_focus_and_stops_at_the_ends() {
    let mut s = strip(&[1, 2, 3]);
    s.scroll_by(-10_000.0);
    assert_eq!(s.scroll(), 0.0);
    assert_eq!(s.focused(), Some(&3));
    s.scroll_by(10_000.0);
    assert_eq!(s.scroll(), 495.0);
    s.scroll_by(-100.0);
    s.apply(Action::ScrollLeft);
    assert_eq!(s.scroll(), 395.0 - 980.0 / 3.0);
}

#[test]
fn hand_scrolling_does_nothing_when_everything_fits() {
    let mut s = strip(&[1, 2]);
    s.scroll_by(300.0);
    assert_eq!(rect(&s, 1).x, 10);
}

#[test]
fn center_focused_once() {
    let mut s = strip(&[1, 2, 3, 4]);
    s.focus(&2);
    s.apply(Action::CenterFocused);
    let r = rect(&s, 2);
    assert_eq!(r.x + r.w / 2, 500);
}

#[test]
fn a_new_screen_size_keeps_each_columns_share() {
    let mut s = strip(&[1, 2]);
    s.set_area(Rect::new(0, 40, 2000, 1000));
    // The view grew from 980 to 1980.
    assert_eq!(s.width_of(&1), Some(980));
    assert_eq!(rect(&s, 1), Rect::new(15, 50, 980, 980));
    assert_eq!(rect(&s, 2).y, 50);
}

#[test]
fn the_area_offsets_everything() {
    let mut s = Strip::new(config(), Rect::new(100, 50, 1000, 600));
    s.add(1);
    assert_eq!(rect(&s, 1), Rect::new(358, 60, 485, 580));
}

#[test]
fn focus_column_by_number() {
    let mut s = strip(&[1, 2, 3]);
    s.apply(Action::FocusColumn(0));
    assert_eq!(s.focused(), Some(&1));
    s.apply(Action::FocusColumn(9));
    assert_eq!(s.focused(), Some(&3));
}

#[test]
fn adding_twice_and_removing_strangers_change_nothing() {
    let mut s = strip(&[1, 2]);
    s.add(1);
    assert_eq!(s.len(), 2);
    assert!(!s.remove(&7));
    assert!(!s.focus(&7));
}
