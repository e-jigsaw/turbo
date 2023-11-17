use std::{
    num::NonZeroUsize,
    sync::{Arc, OnceLock},
};

pub type SpanIndex = NonZeroUsize;

pub struct Span {
    // These values won't change after creation:
    pub index: SpanIndex,
    pub parent: Option<SpanIndex>,
    pub start: u64,
    pub ignore_self_time: bool,
    pub category: String,
    pub name: String,
    pub args: Vec<(String, String)>,

    // This might change during writing:
    pub events: Vec<SpanEvent>,

    // These values are computed automatically:
    pub self_end: u64,
    pub self_time: u64,

    // These values are computed when accessed (and maybe deleted during writing):
    pub end: OnceLock<u64>,
    pub nice_name: OnceLock<(String, String)>,
    pub group_name: OnceLock<String>,
    pub max_depth: OnceLock<u32>,
    pub total_time: OnceLock<u64>,
    pub corrected_self_time: OnceLock<u64>,
    pub corrected_total_time: OnceLock<u64>,
    pub graph: OnceLock<Vec<SpanGraphEvent>>,
}

#[derive(Copy, Clone)]
pub enum SpanEvent {
    SelfTime { start: u64, end: u64 },
    Child { id: SpanIndex },
}

#[derive(Clone)]
pub enum SpanGraphEvent {
    SelfTime { duration: u64 },
    Child { child: Arc<SpanGraph> },
}

pub struct SpanGraph {
    // These values won't change after creation:
    pub root_spans: Vec<SpanIndex>,
    pub recursive_spans: Vec<SpanIndex>,

    // These values are computed when accessed:
    pub max_depth: OnceLock<u32>,
    pub events: OnceLock<Vec<SpanGraphEvent>>,
    pub self_time: OnceLock<u64>,
    pub total_time: OnceLock<u64>,
    pub corrected_self_time: OnceLock<u64>,
    pub corrected_total_time: OnceLock<u64>,
}
