#[derive(Debug, Clone, PartialEq, Eq)]
/// A message for the user.
pub enum Notice {
    Fail(String),
    Say(String),
}

impl Notice {
    #[inline]
    pub fn fail<IntoString>(text: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        Self::Fail(text.into())
    }

    #[inline]
    pub fn say<IntoString>(text: IntoString) -> Self
    where
        IntoString: Into<String>,
    {
        Self::Say(text.into())
    }
}
