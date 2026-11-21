// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's desk (ai-runtime-chain 20.3, 20.5): the fallback question of the `ask` policy,
//! waiting for the owner's answer (one at a time), the tool-use notices and the provenance of the
//! last requests, for the window to read. Only the window's own buttons answer the question.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mailcraft_model::ai::RuntimeId;

use super::super::route::{self, Provenance};
use super::{FallbackQuestion, OwnerAnswer, Watch};

/// How many tool-use notices and provenances are kept for the window to read.
const DESK_KEEPS: usize = 32;
/// How often a waiting walk looks for the owner's answer.
const ANSWER_POLL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct DeskState {
    question: Option<(FallbackQuestion, Option<OwnerAnswer>)>,
    notices: VecDeque<(u64, RuntimeId)>,
    provenances: VecDeque<Provenance>,
}

/// What the walks tell the window and what they ask the owner, for the window to read: the
/// waiting fallback question (one at a time), the tool-use notices and the provenance of the last
/// requests. The answer to the question is the owner's alone: [`Desk::answer_fallback`] is for
/// the window's buttons, never for a command, the control channel or MCP.
#[derive(Default)]
pub(crate) struct Desk {
    state: Mutex<DeskState>,
}

/// The desktop's desk.
pub(crate) fn desk() -> &'static Desk {
    static DESK: OnceLock<Desk> = OnceLock::new();
    DESK.get_or_init(Desk::default)
}

#[allow(dead_code, reason = "the window reads it once the integrator wires the chain into the window (ai-runtime-chain 20.5, 20.8)")]
impl Desk {
    fn lock(&self) -> std::sync::MutexGuard<'_, DeskState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The fallback question waiting for the owner, if any.
    pub(crate) fn fallback_question(&self) -> Option<FallbackQuestion> {
        self.lock().question.as_ref().filter(|(_, answer)| answer.is_none()).map(|(q, _)| q.clone())
    }

    /// The owner's answer to the question for `request` (the window's buttons only).
    pub(crate) fn answer_fallback(&self, request: u64, yes: bool) -> Result<(), String> {
        let mut state = self.lock();
        match state.question.as_mut() {
            Some((question, answer @ None)) if question.request == request => {
                *answer = Some(if yes { OwnerAnswer::Yes } else { OwnerAnswer::No });
                Ok(())
            }
            _ => Err("no such question is waiting".into()),
        }
    }

    /// The question closed without an answer: a no.
    pub(crate) fn dismiss_fallback(&self) {
        if let Some((_, answer @ None)) = self.lock().question.as_mut() {
            *answer = Some(OwnerAnswer::No);
        }
    }

    /// The runtimes stopped for tool use while `request` ran, in order.
    pub(crate) fn tool_use_notices(&self, request: u64) -> Vec<RuntimeId> {
        self.lock().notices.iter().filter(|(r, _)| *r == request).map(|(_, id)| id.clone()).collect()
    }

    /// The provenance of `request`, once its walk is over.
    pub(crate) fn provenance(&self, request: u64) -> Option<Provenance> {
        self.lock().provenances.iter().rev().find(|p| p.request == request).cloned()
    }

    fn note_tool_use(&self, request: u64, runtime: &RuntimeId) {
        let mut state = self.lock();
        if state.notices.len() >= DESK_KEEPS {
            state.notices.pop_front();
        }
        state.notices.push_back((request, runtime.clone()));
    }

    fn keep_provenance(&self, provenance: &Provenance) {
        let mut state = self.lock();
        if state.provenances.len() >= DESK_KEEPS {
            state.provenances.pop_front();
        }
        state.provenances.push_back(provenance.clone());
    }

    /// Puts `question` (unless another is waiting: then a no) and waits for the owner, at most
    /// `wait`; the question is taken away when the walk stops waiting.
    fn ask(&self, question: &FallbackQuestion, cancel: &AtomicBool, wait: Duration, wake: &dyn Fn()) -> OwnerAnswer {
        {
            let mut state = self.lock();
            if state.question.as_ref().is_some_and(|(_, answer)| answer.is_none()) {
                return OwnerAnswer::No;
            }
            state.question = Some((question.clone(), None));
        }
        wake();
        let started = Instant::now();
        let answer = loop {
            if let Some((_, Some(answer))) = self.lock().question.as_ref().filter(|(q, _)| q == question) {
                break *answer;
            }
            if cancel.load(Ordering::Relaxed) || started.elapsed() >= wait {
                break OwnerAnswer::No;
            }
            std::thread::sleep(ANSWER_POLL);
        };
        let mut state = self.lock();
        if state.question.as_ref().is_some_and(|(q, _)| q == question) {
            state.question = None;
        }
        drop(state);
        wake();
        answer
    }
}

/// The desktop's [`Watch`]: notices, the question and the provenance on `desk`, the window woken
/// for each.
pub(crate) struct DeskWatch {
    desk: &'static Desk,
    wake: Option<egui::Context>,
}

impl DeskWatch {
    pub(crate) fn new(desk: &'static Desk, wake: Option<egui::Context>) -> DeskWatch {
        DeskWatch { desk, wake }
    }

    fn wake(&self) {
        if let Some(ctx) = &self.wake {
            ctx.request_repaint();
        }
    }
}

impl Watch for DeskWatch {
    fn tool_use_stopped(&mut self, request: u64, runtime: &RuntimeId) {
        log::warn!("a {} runtime was stopped for trying to use a tool", runtime.kind().name());
        self.desk.note_tool_use(request, runtime);
        self.wake();
    }

    fn ask_owner(&mut self, question: &FallbackQuestion, cancel: &AtomicBool) -> OwnerAnswer {
        self.desk.ask(question, cancel, route::QUESTION_WAIT, &|| self.wake())
    }

    fn provenance(&mut self, provenance: &Provenance) {
        self.desk.keep_provenance(provenance);
        self.wake();
    }
}
