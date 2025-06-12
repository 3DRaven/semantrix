use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumIter, EnumString};

#[derive(Debug, strum_macros::Display)]
pub enum McpProgressToken {
    #[strum(serialize = "mcpLspBridge/symbol")]
    Symbol,
    #[strum(serialize = "rustAnalyzer/cachePriming")]
    CachePriming,
    #[strum(serialize = "rustAnalyzer/Roots Scanned")]
    RootsScanned,
    #[strum(serialize = "rustAnalyzer/Loading proc-macros")]
    LoadingProcMacros,
}

#[derive(Debug, strum_macros::Display, strum_macros::VariantNames)]
pub enum McpPromptName {
    #[strum(serialize = "Fuzzy search implemented stuff by documentation and names parts")]
    FuzzySearchImplementedStuff,
}

#[derive(Debug, strum_macros::Display, strum_macros::VariantNames)]
pub enum McpPromptArgument {
    #[strum(serialize = "short_description")]
    ShortDescription,
    #[strum(serialize = "possible_names")]
    PossibleNames,
}

#[repr(i32)]
#[derive(
    Eq, PartialEq, Copy, Clone, Serialize, Deserialize, Debug, EnumString, EnumIter, Display,
)]
pub enum McpSymbolKind {
    File = 1,
    Module = 2,
    Namespace = 3,
    Package = 4,
    Class = 5,
    Method = 6,
    Property = 7,
    Field = 8,
    Constructor = 9,
    Enum = 10,
    Interface = 11,
    Function = 12,
    Variable = 13,
    Constant = 14,
    String = 15,
    Number = 16,
    Boolean = 17,
    Array = 18,
    Object = 19,
    Key = 20,
    Null = 21,
    EnumMember = 22,
    Struct = 23,
    Event = 24,
    Operator = 25,
    TypeParameter = 26,
}
