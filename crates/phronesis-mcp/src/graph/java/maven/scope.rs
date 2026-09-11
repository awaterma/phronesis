//! Maven's scope propagation table, separate from reactor name resolution.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    #[default]
    Compile,
    Provided,
    Runtime,
    Test,
    System,
    Unsupported,
}

impl Scope {
    pub(super) fn parse(value: &str) -> Self {
        match value {
            "" | "compile" => Self::Compile,
            "provided" => Self::Provided,
            "runtime" => Self::Runtime,
            "test" => Self::Test,
            "system" => Self::System,
            _ => Self::Unsupported,
        }
    }

    pub(super) fn on_compile_classpath(self, test: bool) -> bool {
        matches!(self, Self::Compile | Self::Provided)
            || (test && matches!(self, Self::Runtime | Self::Test))
    }

    pub(super) fn transitive(self, child: Self) -> Option<Self> {
        match (self, child) {
            (Self::Compile, Self::Compile) => Some(Self::Compile),
            (Self::Compile | Self::Runtime, Self::Runtime) | (Self::Runtime, Self::Compile) => {
                Some(Self::Runtime)
            }
            (Self::Provided, Self::Compile | Self::Runtime) => Some(Self::Provided),
            (Self::Test, Self::Compile | Self::Runtime) => Some(Self::Test),
            _ => None,
        }
    }
}
