use super::events::Entry;

use std::{collections::HashMap, sync::Arc};

pub(crate) struct Registry {
    pub(crate) runs: HashMap<String, Arc<Entry>>,
    pub(crate) active: HashMap<String, String>,
    pub(crate) keys: HashMap<(String, String), (String, String)>, // (session, client key) -> (run, input)
}
