use crate::runs::RunManager;

use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub instance_id: String,
    pub runs: Arc<RunManager>,
}
