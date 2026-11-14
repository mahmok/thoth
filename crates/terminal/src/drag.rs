use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

/// Something a left-button drag is over.
pub trait Dragged {
    /// What the drag produces when the button comes up.
    type Output;

    /// Whether a press at `ev` lands on this.
    fn contains(&self, ev: MouseEvent) -> bool;

    /// The button went down on this.
    fn pressed(&mut self, ev: MouseEvent);

    /// The pointer moved while the button was down.
    fn moved(&mut self, ev: MouseEvent);

    /// The button came up, ending the drag.
    fn released(&mut self, ev: MouseEvent) -> Self::Output;
}

/// A left-button drag over whatever `T` drags.
///
/// A press inside starts the drag, moves carry it on, and the release ends it and yields its
/// result. Every other mouse event, including other buttons and the wheel, is not part of one.
#[derive(Default)]
pub struct Drag {
    dragging: bool,
}

impl Drag {
    /// Feeds `ev` to `target`, when it is part of a left-button drag.
    pub fn handle<T: Dragged>(&mut self, ev: MouseEvent, target: &mut T) -> Option<T::Output> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) if target.contains(ev) => {
                self.dragging = true;
                target.pressed(ev);
                None
            },
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                target.moved(ev);
                None
            },
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                Some(target.released(ev))
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    #[derive(Default)]
    struct Fake {
        inside: bool,
        moved: usize,
        pressed: usize,
        released: usize,
    }

    impl Dragged for Fake {
        type Output = usize;

        fn contains(&self, _ev: MouseEvent) -> bool {
            self.inside
        }

        fn pressed(&mut self, _ev: MouseEvent) {
            self.pressed = self.pressed.saturating_add(1);
        }

        fn moved(&mut self, _ev: MouseEvent) {
            self.moved = self.moved.saturating_add(1);
        }

        fn released(&mut self, _ev: MouseEvent) -> usize {
            self.released = self.released.saturating_add(1);
            self.pressed
        }
    }

    fn mouse(kind: MouseEventKind) -> MouseEvent {
        MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE }
    }

    fn left(kind: fn(MouseButton) -> MouseEventKind) -> MouseEvent {
        mouse(kind(MouseButton::Left))
    }

    #[test]
    fn a_left_press_inside_starts_a_drag_that_the_release_ends() {
        let mut drag = Drag::default();
        let mut fake = Fake { inside: true, ..Fake::default() };
        assert_eq!(drag.handle(left(MouseEventKind::Down), &mut fake), None);
        assert_eq!(drag.handle(left(MouseEventKind::Drag), &mut fake), None);
        assert_eq!(drag.handle(left(MouseEventKind::Up), &mut fake), Some(1), "the release produces the result");
        assert_eq!((fake.pressed, fake.moved, fake.released), (1, 1, 1));
        assert_eq!(drag.handle(left(MouseEventKind::Up), &mut fake), None, "the drag is over");
    }

    #[test]
    fn a_drag_never_starts_outside_the_target() {
        let mut drag = Drag::default();
        let mut fake = Fake::default();
        assert_eq!(drag.handle(left(MouseEventKind::Down), &mut fake), None);
        assert_eq!(drag.handle(left(MouseEventKind::Drag), &mut fake), None);
        assert_eq!(drag.handle(left(MouseEventKind::Up), &mut fake), None);
        assert_eq!((fake.pressed, fake.moved, fake.released), (0, 0, 0));
    }

    #[test]
    fn other_buttons_and_the_wheel_are_not_drags() {
        let mut drag = Drag::default();
        let mut fake = Fake { inside: true, ..Fake::default() };
        for kind in [
            mouse(MouseEventKind::Down(MouseButton::Right)),
            mouse(MouseEventKind::Down(MouseButton::Middle)),
            mouse(MouseEventKind::Moved),
            mouse(MouseEventKind::ScrollDown),
            mouse(MouseEventKind::ScrollUp),
            mouse(MouseEventKind::ScrollLeft),
            mouse(MouseEventKind::ScrollRight),
        ] {
            assert_eq!(drag.handle(kind, &mut fake), None, "{kind:?}");
        }
        assert_eq!((fake.pressed, fake.moved, fake.released), (0, 0, 0));
    }
}
