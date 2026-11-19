//! Bounded, room-scoped reads for the search overlay. Only the conversation loop merges them.

use std::{future::Future, pin::Pin};

use openpickle_api::{CardState, Error, HistoryItem, HistoryPage, PickleClient};

use super::{Conversation, Live, explain, session_is_gone};
use crate::{
    convert,
    model::{Message, SearchResult},
    protocol::{SearchRequest, Update},
};

pub(super) enum SearchJob {
    Query(String),
    Open(u64),
}

pub(super) enum SearchReply {
    Found(Result<Vec<SearchResult>, Error>),
    Opened(u64, Result<Option<SearchContext>, Error>),
}

type InFlight = Pin<Box<dyn Future<Output = (SearchRequest, SearchReply)> + Send>>;

/// Keep at most one read in flight and at most the newest waiting intent. Obsolete results are
/// discarded, but the active read can finish any shared credential refresh it has entered.
#[derive(Default)]
pub(super) struct SearchReads {
    active: Option<InFlight>,
    queued: Option<(SearchRequest, SearchJob)>,
    wanted: bool,
    started: bool,
}

impl SearchReads {
    pub(super) fn queue(&mut self, request: SearchRequest, job: SearchJob) {
        self.cancel();
        self.queued = Some((request, job));
    }

    pub(super) fn cancel(&mut self) {
        self.wanted = false;
        self.queued = None;
        if !self.started {
            self.active = None;
        }
    }

    pub(super) fn start_next(&mut self, client: &PickleClient, room_id: &str) {
        if self.active.is_some() {
            return;
        }
        let Some((request, job)) = self.queued.take() else {
            return;
        };
        if request.room_id != room_id || room_id.is_empty() {
            return;
        }
        self.wanted = true;
        let client = client.clone();
        self.active = Some(Box::pin(async move {
            let reply = match job {
                SearchJob::Query(query) => {
                    SearchReply::Found(client.search_messages(&query).await.map(|found| {
                        found
                            .items
                            .into_iter()
                            .map(|item| convert::search_result(item, &request.room_id))
                            .collect()
                    }))
                }
                SearchJob::Open(seq) => {
                    SearchReply::Opened(seq, lookup(&client, &request.room_id, seq).await)
                }
            };
            (request, reply)
        }));
    }

    pub(super) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(super) async fn next(&mut self) -> (SearchRequest, SearchReply, bool) {
        let (request, reply) = match &mut self.active {
            Some(active) => {
                self.started = true;
                active.await
            }
            None => std::future::pending().await,
        };
        self.active = None;
        self.started = false;
        (request, reply, std::mem::take(&mut self.wanted))
    }

    pub(super) async fn finish_on_quit(&mut self) {
        if self.started {
            let _ = self.next().await;
        }
    }
}

const CONTEXT_LIMIT: u32 = 32;
const MISSING: &str =
    "This message is no longer available in this conversation. Choose another result.";

pub(super) struct SearchContext {
    items: Vec<HistoryItem>,
    card_states: Vec<CardState>,
}

/// No scan from the cached tail: history/page is exclusive, so target + 1 includes the target.
/// Ask for newer neighbors independently. Neither result represents contiguous cache coverage.
async fn lookup(
    client: &PickleClient,
    room_id: &str,
    seq: u64,
) -> Result<Option<SearchContext>, Error> {
    let Some(target) = i64::try_from(seq).ok().filter(|seq| *seq > 0) else {
        return Ok(None);
    };
    let before = target.saturating_add(1);
    // At the signed limit, the backward page cannot contain the target. The forward read can.
    let after = if before == target { target - 1 } else { target };
    let (backward, forward) = tokio::join!(
        client.get_history_page(room_id, before, CONTEXT_LIMIT),
        client.get_history_delta(room_id, after, Some(CONTEXT_LIMIT)),
    );
    let newer = match forward {
        Ok(delta) => Some(delta),
        Err(error)
            if before == target
                || session_is_gone(&error)
                || error
                    .api()
                    .is_some_and(|api| matches!(api.status, 403 | 404)) =>
        {
            return Err(error);
        }
        Err(_) => None,
    };
    let backward = backward?;
    let (newer_items, newer_cards) = newer
        .map(|delta| (delta.items, delta.card_states))
        .unwrap_or_default();
    Ok(SearchContext::around(
        target,
        before,
        backward,
        newer_items,
        newer_cards,
    ))
}

impl SearchContext {
    fn around(
        target: i64,
        before: i64,
        older: HistoryPage,
        newer_items: Vec<HistoryItem>,
        newer_cards: Vec<CardState>,
    ) -> Option<Self> {
        let mut items: Vec<_> = older
            .messages
            .into_iter()
            .filter(|item| item.seq > 0 && item.seq < before)
            .take(CONTEXT_LIMIT as usize)
            .collect();
        items.extend(
            newer_items
                .into_iter()
                .filter(|item| item.seq >= before)
                .take(CONTEXT_LIMIT as usize),
        );
        items.iter().any(|item| item.seq == target).then(|| Self {
            items,
            card_states: older.card_states.into_iter().chain(newer_cards).collect(),
        })
    }
}

impl Conversation {
    fn apply_search_context(&mut self, context: SearchContext) -> Vec<Message> {
        let mut ids = Vec::with_capacity(context.items.len());
        for item in context.items {
            let id = item.id.clone();
            ids.push(id.clone());
            // A concurrent live/delta update wins over a slow snapshot with the same ID.
            if let std::collections::hash_map::Entry::Vacant(entry) = self.items.entry(id.clone()) {
                entry.insert(item);
                self.search_context.insert(id);
            }
        }
        for state in context.card_states {
            if self.search_context.contains(&state.id) && ids.contains(&state.id) {
                self.card_states.entry(state.id.clone()).or_insert(state);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids.iter()
            .filter_map(|id| self.items.get(id))
            .map(|item| self.message(item))
            .collect()
    }
}

impl Live {
    pub(super) fn finish_search(
        &mut self,
        request: SearchRequest,
        reply: SearchReply,
        wanted: bool,
    ) -> Result<(), Error> {
        let current = wanted && request.room_id == self.conversation.room_id;
        match reply {
            SearchReply::Found(found) => {
                let results = match found {
                    Err(error) if session_is_gone(&error) => return Err(error),
                    _ if !current => return Ok(()),
                    _ => found.map_err(|error| explain(&error)),
                };
                self.send(Update::Found { request, results });
            }
            SearchReply::Opened(seq, context) => {
                let result = match context {
                    Err(error) if session_is_gone(&error) => return Err(error),
                    _ if !current => return Ok(()),
                    Ok(Some(context)) => Ok(self.conversation.apply_search_context(context)),
                    Ok(None) => Err(MISSING.to_owned()),
                    Err(error) if error.api().is_some_and(|api| api.status == 404) => Err(MISSING.to_owned()),
                    Err(error) if error.api().is_some_and(|api| api.status == 403) => Err("This conversation is no longer accessible. Return to Chat or choose another result.".to_owned()),
                    Err(error) => Err(format!("Could not open this message. {} Select the result to retry.", explain(&error))),
                };
                self.send(Update::SearchResultOpened {
                    request,
                    seq,
                    result,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, seq: i64) -> HistoryItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "seq": seq, "at": seq, "role": "user", "text": id,
        }))
        .unwrap()
    }

    fn page(messages: Vec<HistoryItem>) -> HistoryPage {
        HistoryPage {
            v: 1,
            messages,
            reactions: Vec::new(),
            card_states: Vec::new(),
            has_more: true,
            next_before: 1,
        }
    }

    #[test]
    fn context_requires_the_exact_target_even_when_nearby_messages_exist() {
        let absent = SearchContext::around(
            50,
            51,
            page(vec![item("too-new", 51), item("older", 44)]),
            vec![item("wrong-direction", 50), item("newer", 57)],
            Vec::new(),
        );
        assert!(absent.is_none());

        let at_limit = SearchContext::around(
            i64::MAX,
            i64::MAX,
            page(vec![item("older", i64::MAX - 2)]),
            vec![item("target", i64::MAX)],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(at_limit.items.len(), 2);
    }

    #[test]
    fn sparse_search_context_does_not_move_history_cursors_or_replace_known_items() {
        let mut conversation = Conversation {
            synced: 120,
            ..Default::default()
        };
        conversation.apply(vec![item("known", 120)], Vec::new());
        let context = SearchContext::around(
            50,
            51,
            page(vec![item("target", 50), item("older", 44)]),
            vec![item("known", 119), item("ahead", 121)],
            Vec::new(),
        )
        .unwrap();
        let hydrated = conversation.apply_search_context(context);
        assert!(hydrated.iter().any(|message| message.seq == 50));
        assert_eq!(conversation.items["known"].seq, 120);
        assert_eq!(conversation.first_seq(), Some(120));
        assert_eq!(conversation.head_seq(), 120);
        assert_eq!(conversation.synced, 120);

        // A normal history page can still cover and persist the same items later.
        conversation.apply(vec![item("older", 44), item("ahead", 121)], Vec::new());
        assert_eq!(conversation.first_seq(), Some(44));
        assert_eq!(conversation.head_seq(), 121);
        assert!(!conversation.search_context.contains("older"));
    }

    #[tokio::test]
    async fn cancel_discards_an_active_read_and_coalesces_waiting_searches() {
        let (settle, outcome) = tokio::sync::oneshot::channel();
        let request = |id| SearchRequest {
            room_id: "room".into(),
            id,
        };
        let first_request = request(1);
        let mut search = SearchReads {
            active: Some(Box::pin(async move {
                outcome.await.unwrap();
                (first_request, SearchReply::Found(Ok(Vec::new())))
            })),
            wanted: true,
            ..Default::default()
        };
        {
            let active = std::pin::pin!(search.next());
            assert!(futures::poll!(active).is_pending());
        }
        search.queue(request(2), SearchJob::Query("middle".into()));
        search.queue(request(3), SearchJob::Open(50));
        assert_eq!(search.queued.as_ref().unwrap().0.id, 3);
        search.cancel();
        assert!(search.queued.is_none());
        assert!(search.is_active());
        settle.send(()).unwrap();
        let (completed, _, wanted) = search.next().await;
        assert_eq!(completed.id, 1);
        assert!(!wanted);
    }
}
