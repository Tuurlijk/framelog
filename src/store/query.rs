#[derive(Clone, Debug)]
pub struct FlagTransitionQuery<'a> {
    pub from_ms: i64,
    pub to_ms: i64,
    pub limit: i64,
    pub device_pci: Option<&'a str>,
    pub flag_name: Option<&'a str>,
    pub direction: Option<&'a str>,
    pub newest_first: bool,
}

#[derive(Clone, Debug)]
pub struct ContextTransitionQuery<'a> {
    pub from_ms: i64,
    pub to_ms: i64,
    pub limit: i64,
    pub source_id: Option<&'a str>,
    pub key: Option<&'a str>,
    pub newest_first: bool,
}
