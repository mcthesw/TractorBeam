pub const FRAME_ID_WINDOW_BITS: u64 = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DuplicateDecision {
    New,
    Reordered,
    Duplicate,
    TooOld,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameIdWindow {
    highest: Option<u64>,
    seen: u128,
}

impl FrameIdWindow {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            highest: None,
            seen: 0,
        }
    }

    #[must_use]
    pub const fn highest(&self) -> Option<u64> {
        self.highest
    }

    pub fn observe(&mut self, frame_id: u64) -> DuplicateDecision {
        let Some(highest) = self.highest else {
            self.highest = Some(frame_id);
            self.seen = 1;
            return DuplicateDecision::New;
        };

        if frame_id > highest {
            let advance = frame_id - highest;
            self.seen = if advance >= FRAME_ID_WINDOW_BITS {
                1
            } else {
                (self.seen << advance) | 1
            };
            self.highest = Some(frame_id);
            return DuplicateDecision::New;
        }

        let age = highest - frame_id;
        if age >= FRAME_ID_WINDOW_BITS {
            return DuplicateDecision::TooOld;
        }
        let mask = 1_u128 << age;
        if self.seen & mask != 0 {
            DuplicateDecision::Duplicate
        } else {
            self.seen |= mask;
            DuplicateDecision::Reordered
        }
    }
}
