use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_lite::FutureExt;
use fuzzy_match::match_indices_case_insensitive;
use warpui::{AppContext, Entity};

use super::search_item::SearchItem;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
use crate::terminal::claude_sessions_index::{self, ClaudeSessionEntry};

/// Typing re-runs the query on every keystroke; reuse one scan of the transcripts for this long so
/// filtering stays cheap while the list still reflects new conversations each time it is opened.
const CACHE_TTL: Duration = Duration::from_secs(5);

type Cache = Arc<Mutex<Option<(Instant, Arc<Vec<ClaudeSessionEntry>>)>>>;

/// Lists recent Claude Code conversations. Transcripts are read inside the returned future, which
/// the mixer runs on a background thread, never on the UI thread.
#[derive(Default)]
pub struct DataSource {
    cache: Cache,
}

impl DataSource {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Entity for DataSource {
    type Event = ();
}

impl AsyncDataSource for DataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        _app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        let query_text = query.text.trim().to_string();
        let cache = self.cache.clone();
        async move {
            let entries = cached_or_scan(&cache);
            Ok(search(&entries, &query_text))
        }
        .boxed()
    }
}

fn cached_or_scan(cache: &Cache) -> Arc<Vec<ClaudeSessionEntry>> {
    if let Ok(guard) = cache.lock()
        && let Some((scanned_at, entries)) = guard.as_ref()
        && scanned_at.elapsed() < CACHE_TTL
    {
        return entries.clone();
    }
    let entries = Arc::new(
        claude_sessions_index::projects_dir()
            .map(|dir| claude_sessions_index::recent_sessions(&dir, claude_sessions_index::DEFAULT_LIMIT))
            .unwrap_or_default(),
    );
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), entries.clone()));
    }
    entries
}

/// Every entry, newest first, for an empty query; otherwise the entries whose title or directory
/// fuzzy-match it, best match first and newest first among equals.
fn search(entries: &[ClaudeSessionEntry], query: &str) -> Vec<QueryResult<CommandPaletteItemAction>> {
    let home = dirs::home_dir();
    let total = entries.len();
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let display_cwd = claude_sessions_index::display_cwd(&entry.cwd, home.as_deref());
            // Recency breaks ties: the newest entry gets the largest bonus, below one fuzzy point.
            let recency = (total - index) as f64 / (total + 1) as f64;
            if query.is_empty() {
                return Some(SearchItem::new(entry.clone(), display_cwd, recency, Vec::new()));
            }
            let title_match = match_indices_case_insensitive(&entry.title, query);
            let cwd_match = match_indices_case_insensitive(&display_cwd, query);
            let (score, title_indices) = match (title_match, cwd_match) {
                (Some(title), Some(cwd)) if cwd.score > title.score => (cwd.score, Vec::new()),
                (Some(title), _) => (title.score, title.matched_indices),
                (None, Some(cwd)) => (cwd.score, Vec::new()),
                (None, None) => return None,
            };
            Some(SearchItem::new(
                entry.clone(),
                display_cwd,
                score as f64 + recency,
                title_indices,
            ))
        })
        .map(QueryResult::from)
        .collect()
}
