//! Declared side-effect categories used by tool authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOperation {
    Read,
    State,
    Write,
    Command,
    Network,
    External,
}
impl ToolOperation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "Read files",
            Self::State => "Update execution state",
            Self::Write => "Modify files",
            Self::Command => "Run command",
            Self::Network => "Network search",
            Self::External => "External or unclassified tool",
        }
    }
}
