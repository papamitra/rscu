use crate::control::MouseButton;
use smithay::utils::{Logical, Point, Rectangle};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub enum Action {
    Move,
    Click { button: MouseButton, double: bool },
    Scroll { horizontal: i32, vertical: i32 },
    Drag { end: Point<i32, Logical> },
}

pub enum Event {
    Press(MouseButton),
    Release,
    Move(Point<i32, Logical>),
    Scroll { horizontal: i32, vertical: i32 },
}

pub struct Sequence {
    events: VecDeque<(u64, Event)>,
    deadline: Instant,
    pub held: Option<MouseButton>,
    pub epoch: u64,
    pub window: u64,
    pub window_area: Rectangle<i32, Logical>,
}

impl Sequence {
    pub fn new(
        action: Action,
        point: Point<i32, Logical>,
        epoch: u64,
        window: u64,
        window_area: Rectangle<i32, Logical>,
    ) -> Option<Self> {
        let mut events = VecDeque::new();
        match action {
            Action::Move => return None,
            Action::Click { button, double } => {
                events.extend([(16, Event::Press(button)), (45, Event::Release)]);
                if double {
                    events.extend([(55, Event::Press(button)), (45, Event::Release)]);
                }
            }
            Action::Scroll {
                horizontal,
                vertical,
            } => {
                events.push_back((
                    16,
                    Event::Scroll {
                        horizontal,
                        vertical,
                    },
                ));
            }
            Action::Drag { end } => {
                events.push_back((16, Event::Press(MouseButton::Left)));
                let start = point.to_f64();
                let delta = end.to_f64() - start;
                for step in 1..=12 {
                    let ratio = f64::from(step) / 12.0;
                    let next = Point::from((start.x + delta.x * ratio, start.y + delta.y * ratio))
                        .to_i32_round();
                    events.push_back((20, Event::Move(next)));
                }
                events.push_back((20, Event::Release));
            }
        }
        Some(Self {
            deadline: Instant::now() + Duration::from_millis(events[0].0),
            events,
            held: None,
            epoch,
            window,
            window_area,
        })
    }

    pub fn time_until_step(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub fn next(&mut self) -> Option<Event> {
        if !self.time_until_step().is_zero() {
            return None;
        }
        let (_, event) = self.events.pop_front()?;
        if let Some((delay, _)) = self.events.front() {
            self.deadline = Instant::now() + Duration::from_millis(*delay);
        }
        Some(event)
    }

    pub fn complete(&self) -> bool {
        self.events.is_empty()
    }

    pub fn only_release_left(&self) -> bool {
        matches!(self.events.front(), Some((_, Event::Release))) && self.events.len() == 1
    }
}
