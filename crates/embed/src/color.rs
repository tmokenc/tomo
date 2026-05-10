/// Theme colours used everywhere by the bot. Values follow `tomoka-rs`'s
/// long-standing palette so the look-and-feel is familiar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Information,
    Success,
    Error,
    Warning,
    MessageUpdate,
    MessageDelete,
    Lovely,
    Custom(u32),
}

impl Color {
    pub const fn value(self) -> u32 {
        match self {
            Self::Information => 0x9966ff,
            Self::Success => 0x3cb371,
            Self::Error => 0xff0033,
            Self::Warning => 0xffaa00,
            Self::MessageUpdate => 0x4edb5f,
            Self::MessageDelete => 0xdb5f4e,
            Self::Lovely => 0xfc2368,
            Self::Custom(v) => v,
        }
    }
}

impl From<Color> for u32 {
    fn from(value: Color) -> Self {
        value.value()
    }
}

impl From<u32> for Color {
    fn from(value: u32) -> Self {
        Self::Custom(value)
    }
}
