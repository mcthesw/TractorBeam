use std::fmt::{self, Display};

mod session;

pub(crate) use session::*;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PeerId(u64);

impl PeerId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl Display for PeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "peer-{}", self.0)
    }
}
