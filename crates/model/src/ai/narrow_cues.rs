// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find by Description, the part that needs no model: what the owner's own words say plainly,
//! read by fixed rules, so that a small model's search is held to it ([`Cues`]).
//!
//! - **Read from the owner's words only**, and today's date: nothing of the mail, and nothing
//!   is sent anywhere. The rules read a sender (`from Ada Abernathy`, `Greta sent me`,
//!   `not from Ada`), a recipient (`sent to Bob`), dates (`since 21 June`, `last Tuesday`,
//!   `in March`, `before 2026-10-01`, `last week`), attachments (`with an attachment`,
//!   `with a PDF`, `without attachments`), `unread` and `flagged`, and the words that only
//!   describe the asking (`mail`, `anything`, `notes that mention`, `sent me`).
//! - **Only what is unambiguous is read.** A name is a word in capitals (or an address), never a
//!   month, a day of the week or a word that asks; two different senders, or a date that could be
//!   the subject of the mail, read as nothing, and the model's search stands.
//! - **The model's search is refined, not replaced** ([`Cues::refine`]): its terms of a kind a
//!   rule read are swapped for the rule's, and its plain words that only describe the asking are
//!   dropped. What is left is checked against the query language like any proposal
//!   ([`super::narrow::check_query`]); when nothing is left, the owner's own remaining words are
//!   the search ([`Cues::fallback`]).
//! - Bounded: at most [`MAX_WORDS`] words of the description are read, and every loop is over them.

use crate::search::{is_address, keyword_value};
use mailcraft_time::{Date, Weekday, days_in_month};

/// Most words of a description the rules read.
pub const MAX_WORDS: usize = 200;
/// Most words a name runs to (`Ada de la Cruz` is four).
const MAX_NAME_WORDS: usize = 4;
/// Most plain words a fallback search keeps.
const MAX_FALLBACK_WORDS: usize = 8;

/// Words that begin a request and only describe it: `find me all the mail…`.
const LEAD: &[&str] = &["find", "show", "search", "look", "get", "list", "give", "me", "for", "all", "any", "the", "my", "every"];
/// What a request asks for, which every message is: `mail`, `anything`.
const HEADS: &[&str] = &[
    "mail",
    "mails",
    "email",
    "emails",
    "e-mail",
    "e-mails",
    "message",
    "messages",
    "note",
    "notes",
    "anything",
    "everything",
    "something",
    "stuff",
    "conversation",
    "conversations",
    "thread",
    "threads",
    "letter",
    "letters",
    "correspondence",
    "ones",
];
/// Words that tie what is asked for to what it is about: `notes that mention`, `about`.
const LINKS: &[&str] = &["about", "regarding", "concerning", "mentioning", "containing", "discussing", "referencing", "including", "re"];
/// The verbs of `that mention…`.
const CLAUSE_VERBS: &[&str] = &[
    "mention",
    "mentions",
    "mentioned",
    "talk",
    "talks",
    "discuss",
    "discusses",
    "discussed",
    "say",
    "says",
    "said",
    "contain",
    "contains",
    "include",
    "includes",
    "reference",
    "references",
    "refer",
    "refers",
    "are",
    "is",
    "were",
    "was",
];
/// The asker, and asking: never what the mail says.
const ASKER: &[&str] = &["i", "me", "my", "us", "we", "our", "got", "please", "something", "anything", "everything"];
/// Heads of a request that ask for mail with attachments: `files Nora sent`.
const ATTACHMENT_HEADS: &[&str] = &["attachments", "files"];
/// Small words that find nothing on their own.
const STOP: &[&str] = &[
    "a", "an", "the", "of", "for", "and", "or", "to", "in", "on", "at", "by", "is", "are", "was", "were", "be", "it", "this", "that", "these",
    "those", "some", "any", "all", "with", "which", "who", "what",
];
/// Words that bound what is asked for, without a date or a name after them to say how.
const BOUNDS: &[&str] = &["from", "since", "sent", "before", "after", "until", "till", "during", "between", "not", "except"];
/// Words that end a name: what comes after a sender in a request.
const NAME_STOPS: &[&str] = &[
    "since",
    "before",
    "after",
    "until",
    "till",
    "on",
    "in",
    "during",
    "between",
    "from",
    "to",
    "cc",
    "with",
    "without",
    "about",
    "regarding",
    "that",
    "which",
    "who",
    "where",
    "when",
    "and",
    "or",
    "but",
    "not",
    "last",
    "this",
    "past",
    "today",
    "yesterday",
    "sent",
    "wrote",
    "mentioning",
    "containing",
    "concerning",
    "attached",
    "having",
    "has",
    "have",
    "me",
    "my",
    "i",
    "us",
    "our",
    "the",
    "a",
    "an",
    "unread",
    "flagged",
];
/// File kinds a request names for an attachment (`with a PDF`).
const FILE_KINDS: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "zip", "csv", "jpg", "jpeg", "png", "txt", "ics"];
/// Months by full name and three or four letters.
const MONTHS: &[(&str, u8)] = &[
    ("january", 1),
    ("jan", 1),
    ("february", 2),
    ("feb", 2),
    ("march", 3),
    ("mar", 3),
    ("april", 4),
    ("apr", 4),
    ("may", 5),
    ("june", 6),
    ("jun", 6),
    ("july", 7),
    ("jul", 7),
    ("august", 8),
    ("aug", 8),
    ("september", 9),
    ("sept", 9),
    ("sep", 9),
    ("october", 10),
    ("oct", 10),
    ("november", 11),
    ("nov", 11),
    ("december", 12),
    ("dec", 12),
];
/// The days `received:` names in words, as the owner writes them.
const SPANS: &[(&str, &str)] = &[
    ("this week", "\"this week\""),
    ("last week", "\"last week\""),
    ("this month", "\"this month\""),
    ("last month", "\"last month\""),
    ("this year", "\"this year\""),
];

/// One word of the description: as written (without the punctuation round it), in small
/// letters, and whether punctuation ended it (a comma ends a name).
#[derive(Clone, Debug)]
struct Word {
    text: String,
    lower: String,
    ends: bool,
    /// Written with `'s` (`Dmitri's`), which is dropped from `text`.
    possessive: bool,
}

/// The words of `description`, at most [`MAX_WORDS`].
fn words(description: &str) -> Vec<Word> {
    description
        .split_whitespace()
        .take(MAX_WORDS)
        .filter_map(|raw| {
            let ends = raw.ends_with([',', ';', '.', '!', '?', ':', ')']);
            let text = raw.trim_matches(|c: char| !c.is_alphanumeric());
            let bare = text.strip_suffix("'s").or_else(|| text.strip_suffix("\u{2019}s"));
            let possessive = bare.is_some();
            let text = bare.unwrap_or(text).to_string();
            (!text.is_empty()).then(|| Word { lower: text.to_lowercase(), text, ends, possessive })
        })
        .collect()
}

/// What the owner's words say plainly (see the module's documentation).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cues {
    /// `from:` terms (`-from:` for `not from`), as they are written.
    sender: Vec<String>,
    /// `to:` terms.
    recipient: Vec<String>,
    /// Date terms (`received:>=2026-06-21`, `on:2026-11-17`).
    dates: Vec<String>,
    /// `has:attachment`, `-has:attachment` or `attachment:pdf`.
    attachment: Option<String>,
    /// `is:unread`, `is:flagged`.
    state: Vec<String>,
    /// The description's words (small letters) that the terms above, or the asking, explain.
    consumed: Vec<String>,
    /// The description's words in small letters, for [`Cues::fallback`].
    rest: Vec<String>,
}

impl Cues {
    /// What `description` says plainly, with `today` for the dates it names.
    pub fn read(description: &str, today: Date) -> Cues {
        let w = words(description);
        let mut cues = Cues::default();
        let mut used = vec![false; w.len()];
        cues.read_asking(&w, &mut used);
        cues.read_attachment(&w, &mut used);
        cues.read_dates(&w, today, &mut used);
        cues.read_people(&w, &mut used);
        cues.read_state(&w, &mut used);
        cues.consumed = w.iter().zip(&used).filter(|(_, u)| **u).map(|(w, _)| w.lower.clone()).collect();
        cues.rest = w.iter().zip(&used).filter(|(_, u)| !**u).map(|(w, _)| w.lower.clone()).collect();
        cues
    }

    /// The terms the rules read, in the order a person would write them.
    pub fn terms(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.sender.iter().cloned());
        out.extend(self.recipient.iter().cloned());
        out.extend(self.attachment.iter().cloned());
        out.extend(self.state.iter().cloned());
        out.extend(self.dates.iter().cloned());
        out
    }

    /// Whether the rules read nothing that becomes a term.
    pub fn is_empty(&self) -> bool {
        self.terms().is_empty()
    }

    /// `query` (a model's proposal) held to these cues: its terms of a kind a rule read give way
    /// to the rule's, and its plain words that only describe the asking are dropped. Empty when
    /// nothing is left of either.
    pub fn refine(&self, query: &str) -> String {
        let mut out = self.terms();
        for term in raw_terms(query) {
            if self.keeps(&term) {
                out.push(term);
            }
        }
        dedup(out).join(" ")
    }

    /// The search from the owner's own words when the model's gave nothing: the rules' terms and
    /// the words left that aren't small words, at most [`MAX_FALLBACK_WORDS`] of them.
    pub fn fallback(&self) -> String {
        let mut out = self.terms();
        let plain = self
            .rest
            .iter()
            .filter(|w| ![STOP, ASKER, HEADS, LINKS, LEAD, BOUNDS].iter().any(|list| list.contains(&w.as_str())))
            .filter(|w| w.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '.' || c == '@'))
            .take(MAX_FALLBACK_WORDS)
            .cloned();
        out.extend(plain);
        dedup(out).join(" ")
    }

    /// Whether a term of the model's search stays beside these cues.
    fn keeps(&self, term: &str) -> bool {
        let body = term.strip_prefix('-').unwrap_or(term);
        if let Some((key, value)) = keyword(body) {
            return self.keeps_keyword(&key, value);
        }
        let lower = body.trim_matches('"').to_lowercase();
        let parts: Vec<String> = lower.split_whitespace().map(|p| p.trim_matches(|c: char| !c.is_alphanumeric()).to_string()).collect();
        if parts.iter().all(String::is_empty) {
            return false;
        }
        let asked_for_mail = self.consumed.iter().any(|c| HEADS.contains(&c.as_str()));
        !parts.iter().filter(|p| !p.is_empty()).all(|p| {
            self.consumed.contains(p) || STOP.contains(&p.as_str()) || ASKER.contains(&p.as_str()) || asked_for_mail && HEADS.contains(&p.as_str())
        })
    }

    /// Whether the model's `key:value` stays: not when a rule read that kind of term.
    fn keeps_keyword(&self, key: &str, value: &str) -> bool {
        const SENDER: &[&str] = &["from", "sender", "by", "author"];
        const RECIPIENT: &[&str] = &["to", "recipient"];
        const DATE: &[&str] =
            &["since", "until", "till", "date", "dated", "received", "sent", "before", "after", "on", "during", "in", "between", "when", "day"];
        const ATTACH: &[&str] = &["has", "hasattachment", "hasattachments", "with", "attached", "without"];
        if !self.sender.is_empty() && SENDER.contains(&key) || !self.recipient.is_empty() && RECIPIENT.contains(&key) {
            return false;
        }
        if !self.dates.is_empty() && DATE.contains(&key) {
            return false;
        }
        if let Some(cue) = &self.attachment {
            let generic = matches!(value.to_lowercase().as_str(), "yes" | "true" | "any" | "1" | "attachment" | "attachments");
            if ATTACH.contains(&key) || (key == "attachment" || key == "attachments") && (generic || cue.starts_with("attachment:")) {
                return false;
            }
        }
        if key == "is" && self.state.iter().any(|s| s.eq_ignore_ascii_case(&format!("is:{value}"))) {
            return false;
        }
        true
    }

    /// The words that only describe the asking: the head of the request (`find me all the
    /// mail`), `that mention`, `about`, and the asker (`me`, `I`).
    fn read_asking(&mut self, w: &[Word], used: &mut [bool]) {
        let mut i = 0;
        while let Some(word) = w.get(i) {
            let l = word.lower.as_str();
            if ATTACHMENT_HEADS.contains(&l) && self.attachment.is_none() {
                self.attachment = Some("has:attachment".to_string());
            } else if !(LEAD.contains(&l) || HEADS.contains(&l)) {
                break;
            }
            mark(used, i, 1);
            i = i.saturating_add(1);
        }
        for (i, word) in w.iter().enumerate() {
            let l = word.lower.as_str();
            let next = w.get(i.saturating_add(1)).map(|n| n.lower.as_str());
            if (l == "that" || l == "which") && next.is_some_and(|n| CLAUSE_VERBS.contains(&n)) {
                mark(used, i, 2);
                if w.get(i.saturating_add(2)).is_some_and(|n| n.lower == "about") {
                    mark(used, i.saturating_add(2), 1);
                }
            }
            if LINKS.contains(&l) || ASKER.contains(&l) {
                mark(used, i, 1);
            }
        }
    }

    /// `with an attachment`, `with a PDF`, `without attachments`, `with no attachment`, `that
    /// have attachments`, `with a file attached`.
    fn read_attachment(&mut self, w: &[Word], used: &mut [bool]) {
        const ARTICLES: &[&str] = &["a", "an", "any", "some", "no"];
        for (i, word) in w.iter().enumerate() {
            let lead = word.lower.as_str();
            if !matches!(lead, "with" | "without" | "has" | "have" | "having" | "had") {
                continue;
            }
            let mut j = i.saturating_add(1);
            let mut negative = lead == "without";
            while let Some(a) = w.get(j).filter(|a| ARTICLES.contains(&a.lower.as_str())) {
                negative |= a.lower == "no";
                j = j.saturating_add(1);
            }
            let Some(object) = w.get(j) else { continue };
            let o = object.lower.as_str();
            let attached_after = w.get(j.saturating_add(1)).is_some_and(|n| n.lower == "attached");
            let cue = if matches!(o, "attachment" | "attachments") || matches!(o, "file" | "files" | "something") && attached_after {
                if negative { "-has:attachment".to_string() } else { "has:attachment".to_string() }
            } else if let Some(kind) = FILE_KINDS.iter().find(|k| o.trim_end_matches('s') == **k || o == **k).filter(|_| !negative) {
                format!("attachment:{kind}")
            } else {
                continue;
            };
            if self.attachment.as_ref().is_some_and(|a| *a != cue) {
                self.attachment = None;
                return;
            }
            self.attachment = Some(cue);
            let len = j.saturating_sub(i).saturating_add(if attached_after { 2 } else { 1 });
            mark(used, i, len);
        }
    }

    /// `unread` and `flagged`.
    fn read_state(&mut self, w: &[Word], used: &mut [bool]) {
        for (i, word) in w.iter().enumerate() {
            let term = match word.lower.as_str() {
                "unread" => "is:unread",
                "flagged" => "is:flagged",
                _ => continue,
            };
            if !self.state.iter().any(|s| s == term) {
                self.state.push(term.to_string());
            }
            mark(used, i, 1);
        }
    }

    /// Senders and recipients: `from NAME`, `not from NAME`, `sent by NAME`, `NAME sent me`,
    /// `sent to NAME`, `mail to NAME`. Two different senders read as none.
    fn read_people(&mut self, w: &[Word], used: &mut [bool]) {
        let mut senders: Vec<(String, bool, Vec<usize>)> = Vec::new();
        let mut recipients: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, word) in w.iter().enumerate() {
            let prev = i.checked_sub(1).and_then(|p| w.get(p)).map(|p| p.lower.as_str());
            let after = i.saturating_add(1);
            if word.possessive
                && w.get(after).is_some_and(|n| HEADS.contains(&n.lower.as_str()))
                && let Some((name, start)) = name_before(w, used, after)
            {
                senders.push((name, false, (start..after).collect()));
                continue;
            }
            match word.lower.as_str() {
                "from" if !used.get(i).copied().unwrap_or(true) => {
                    if let Some((name, len)) = name_at(w, used, after) {
                        let not = matches!(prev, Some("not" | "except"));
                        let start = if not { i.saturating_sub(1) } else { i };
                        senders.push((name, not, (start..after.saturating_add(len)).collect()));
                    }
                }
                "by" if matches!(prev, Some("sent" | "written" | "forwarded")) => {
                    if let Some((name, len)) = name_at(w, used, after) {
                        senders.push((name, false, (i.saturating_sub(1)..after.saturating_add(len)).collect()));
                    }
                }
                "to" if matches!(prev, Some("sent" | "wrote" | "written" | "addressed")) || prev.is_some_and(|p| HEADS.contains(&p)) => {
                    if let Some((name, len)) = name_at(w, used, after) {
                        recipients.push((name, (i..after.saturating_add(len)).collect()));
                    }
                }
                "sent" if !matches!(w.get(after).map(|n| n.lower.as_str()), Some("to" | "by")) => {
                    if let Some((name, start)) = name_before(w, used, i) {
                        let mut span: Vec<usize> = (start..=i).collect();
                        if w.get(after).is_some_and(|n| matches!(n.lower.as_str(), "me" | "us")) {
                            span.push(after);
                        }
                        senders.push((name, false, span));
                    }
                }
                _ => {}
            }
        }
        if let Some((name, not, span)) = one(&senders, |s| (s.0.to_lowercase(), s.1)) {
            self.sender.push(format!("{}from:{}", if not { "-" } else { "" }, keyword_value(&name)));
            span.iter().for_each(|&k| mark(used, k, 1));
        }
        if let Some((name, span)) = one(&recipients, |r| r.0.to_lowercase()) {
            self.recipient.push(format!("to:{}", keyword_value(&name)));
            span.iter().for_each(|&k| mark(used, k, 1));
        }
    }

    /// Date terms: `since D`, `after D`, `before D`, `on D`, `until D`, `in MONTH`,
    /// `between D and D`, `from D to D`, `last Tuesday`, `today`, `last week`, `last year`,
    /// `in the last N days`. A date with no word before it to say how it bounds the mail is left
    /// alone: it may be what the mail is about.
    fn read_dates(&mut self, w: &[Word], today: Date, used: &mut [bool]) {
        let mut i = 0;
        while i < w.len() {
            let step = match self.date_at(w, today, i) {
                Some((term, len)) => {
                    if !self.dates.contains(&term) {
                        self.dates.push(term);
                    }
                    mark(used, i, len);
                    len.max(1)
                }
                None => 1,
            };
            i = i.saturating_add(step);
        }
    }

    /// The date term that starts at word `i`, and how many words it takes.
    fn date_at(&self, w: &[Word], today: Date, i: usize) -> Option<(String, usize)> {
        let lead = w.get(i)?.lower.as_str();
        let next = i.saturating_add(1);
        match lead {
            "since" | "after" | "before" | "on" | "until" | "till" | "from" => {
                let (when, len) = when_at(w, today, next)?;
                let (first, last) = when.days()?;
                let term = match lead {
                    "since" => format!("received:>={}", first.iso()),
                    "after" => format!("after:{}", last.iso()),
                    "before" => format!("before:{}", first.iso()),
                    "until" | "till" => format!("received:<={}", last.iso()),
                    "on" if first == last => format!("on:{}", first.iso()),
                    "on" => format!("received:{}..{}", first.iso(), last.iso()),
                    _ => return from_to(w, today, first, next.saturating_add(len)).map(|(t, l)| (t, l.saturating_add(len).saturating_add(1))),
                };
                Some((term, len.saturating_add(1)))
            }
            "in" | "during" => {
                if let Some(found) = recent(w, today, next) {
                    return Some((found.0, found.1.saturating_add(1)));
                }
                let (when, len) = when_at(w, today, next).filter(|(when, _)| !matches!(when, When::Day(_)))?;
                let (first, last) = when.days()?;
                Some((format!("received:{}..{}", first.iso(), last.iso()), len.saturating_add(1)))
            }
            "between" => {
                let (when, len) = when_at(w, today, next)?;
                let and = next.saturating_add(len);
                if w.get(and).is_none_or(|a| a.lower != "and") {
                    return None;
                }
                let (end, len2) = when_at(w, today, and.saturating_add(1))?;
                let (first, last) = (when.days()?.0, end.days()?.1);
                (first <= last).then(|| (format!("received:{}..{}", first.iso(), last.iso()), len.saturating_add(len2).saturating_add(2)))
            }
            "last" | "past" => recent(w, today, i),
            _ => span_at(w, i),
        }
    }
}

/// `from D to D` (or `until`): the range, and the words after the first day it takes; `from D`
/// alone is `since D`.
fn from_to(w: &[Word], today: Date, first: Date, at: usize) -> Option<(String, usize)> {
    let to = w.get(at).filter(|t| matches!(t.lower.as_str(), "to" | "until" | "till"));
    if to.is_none() {
        return Some((format!("received:>={}", first.iso()), 0));
    }
    let (end, len) = when_at(w, today, at.saturating_add(1))?;
    let last = end.days()?.1;
    (first <= last).then(|| (format!("received:{}..{}", first.iso(), last.iso()), len.saturating_add(1)))
}

/// A day, a month or a year the owner named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum When {
    Day(Date),
    Month(i32, u8),
    Year(i32),
}

impl When {
    /// Its first and last days.
    fn days(self) -> Option<(Date, Date)> {
        match self {
            When::Day(d) => Some((d, d)),
            When::Month(y, m) => Some((Date::new(y, m, 1)?, Date::new(y, m, days_in_month(y, m))?)),
            When::Year(y) => Some((Date::new(y, 1, 1)?, Date::new(y, 12, 31)?)),
        }
    }
}

/// The day, month or year named at word `i`, and how many words it takes: `2026-06-21`,
/// `21 June`, `21st of June 2026`, `June 21`, `June 21, 2026`, `June`, `June 2026`, `2026`,
/// `Tuesday`, `yesterday`, `today`. A day without a year is the latest such day not after today.
fn when_at(w: &[Word], today: Date, i: usize) -> Option<(When, usize)> {
    let first = w.get(i)?;
    let l = first.lower.as_str();
    if let Some(d) = Date::parse_iso(l).filter(|_| l.len() == 10) {
        return Some((When::Day(d), 1));
    }
    match l {
        "today" => return Some((When::Day(today), 1)),
        "yesterday" => return Some((When::Day(today.add_days(-1)), 1)),
        _ => {}
    }
    if let Some(day) = weekday(l) {
        return Some((When::Day(latest_weekday(today, day, false)), 1));
    }
    if let Some(day) = day_number(l) {
        let mut j = i.saturating_add(1);
        if w.get(j).is_some_and(|o| o.lower == "of") {
            j = j.saturating_add(1);
        }
        let m = month(&w.get(j)?.lower)?;
        let (year, used) = year_at(w, j.saturating_add(1));
        let date = resolve_day(today, year, m, day)?;
        return Some((When::Day(date), j.saturating_sub(i).saturating_add(1).saturating_add(used)));
    }
    if let Some(m) = month(l) {
        if let Some(day) = w.get(i.saturating_add(1)).and_then(|d| day_number(&d.lower)) {
            let (year, used) = year_at(w, i.saturating_add(2));
            return Some((When::Day(resolve_day(today, year, m, day)?), 2usize.saturating_add(used)));
        }
        let (year, used) = year_at(w, i.saturating_add(1));
        let year = year.unwrap_or(if m <= today.month { today.year } else { today.year.saturating_sub(1) });
        return Some((When::Month(year, m), 1usize.saturating_add(used)));
    }
    year(l).map(|y| (When::Year(y), 1))
}

/// `last Tuesday`, `last year`, `last 7 days`, `the last 2 weeks`, `past month` at word `i`
/// (which is `last` or `past`, or `the` before one); and the days `received:` names in words.
fn recent(w: &[Word], today: Date, i: usize) -> Option<(String, usize)> {
    let mut i = i;
    let mut extra = 0usize;
    if w.get(i).is_some_and(|t| t.lower == "the") {
        i = i.saturating_add(1);
        extra = 1;
    }
    let lead = w.get(i)?.lower.as_str();
    if !matches!(lead, "last" | "past") {
        return span_at(w, i).map(|(t, l)| (t, l.saturating_add(extra)));
    }
    let next = w.get(i.saturating_add(1))?.lower.as_str();
    if let Some(day) = weekday(next) {
        return Some((format!("on:{}", latest_weekday(today, day, true).iso()), extra.saturating_add(2)));
    }
    if next == "year" && lead == "last" {
        let y = today.year.saturating_sub(1);
        return Some((format!("received:{y:04}-01-01..{y:04}-12-31"), extra.saturating_add(2)));
    }
    if let Some(n) = count(next) {
        let unit = w.get(i.saturating_add(2))?.lower.as_str();
        let back = match unit.trim_end_matches('s') {
            "day" => i64::from(n),
            "week" => i64::from(n).saturating_mul(7),
            _ => return None,
        };
        return Some((format!("received:>={}", today.add_days(back.saturating_mul(-1)).iso()), extra.saturating_add(3)));
    }
    if lead == "last" {
        return span_at(w, i).map(|(t, l)| (t, l.saturating_add(extra)));
    }
    None
}

/// `today`, `yesterday`, `this week`, `last week`, `this month`, `last month`, `this year` at
/// word `i`, as `received:` takes them.
fn span_at(w: &[Word], i: usize) -> Option<(String, usize)> {
    let first = w.get(i)?.lower.as_str();
    if matches!(first, "today" | "yesterday") {
        return Some((format!("received:{first}"), 1));
    }
    let second = w.get(i.saturating_add(1))?.lower.as_str();
    let pair = format!("{first} {second}");
    SPANS.iter().find(|(name, _)| *name == pair).map(|(_, value)| (format!("received:{value}"), 2))
}

/// A year after word `j`, if one is there (`2026`), and the words it takes.
fn year_at(w: &[Word], j: usize) -> (Option<i32>, usize) {
    match w.get(j).and_then(|y| year(&y.lower)) {
        Some(y) => (Some(y), 1),
        None => (None, 0),
    }
}

/// `2026`: a four-figure year from 1900 to 2100.
fn year(s: &str) -> Option<i32> {
    (s.len() == 4).then(|| s.parse::<i32>().ok()).flatten().filter(|y| (1900..=2100).contains(y))
}

/// `21`, `21st`, `2nd`: a day of the month.
fn day_number(s: &str) -> Option<u8> {
    let digits = s.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = s.get(digits.len()..).unwrap_or("");
    if digits.is_empty() || digits.len() > 2 || !matches!(suffix, "" | "st" | "nd" | "rd" | "th") {
        return None;
    }
    digits.parse::<u8>().ok().filter(|d| (1..=31).contains(d))
}

/// `7`, `seven`: a small count.
fn count(s: &str) -> Option<u16> {
    const WORDS: &[&str] = &["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];
    if let Some(i) = WORDS.iter().position(|n| *n == s) {
        return u16::try_from(i.saturating_add(1)).ok();
    }
    (s.len() <= 3).then(|| s.parse::<u16>().ok()).flatten().filter(|n| (1..=366).contains(n))
}

/// A month's number from its name (`june`, `jun`, `sept`).
fn month(s: &str) -> Option<u8> {
    MONTHS.iter().find(|(name, _)| *name == s).map(|(_, m)| *m)
}

/// A day of the week from its name or three letters.
fn weekday(s: &str) -> Option<Weekday> {
    if s.len() < 3 || !Weekday::ALL.iter().any(|d| d.name().to_lowercase() == s || d.name().to_lowercase().get(..3) == Some(s)) {
        return None;
    }
    Weekday::parse(s)
}

/// The latest `day` not after `today` (before it, when `strictly`).
fn latest_weekday(today: Date, day: Weekday, strictly: bool) -> Date {
    let back = (i64::from(today.weekday().index()) - i64::from(day.index())).rem_euclid(7);
    let back = if strictly && back == 0 { 7 } else { back };
    today.add_days(back.saturating_mul(-1))
}

/// `month`/`day` of `year`, or without one the latest such day not after `today`.
fn resolve_day(today: Date, year: Option<i32>, month: u8, day: u8) -> Option<Date> {
    if let Some(y) = year {
        return Date::new(y, month, day);
    }
    (0..8).filter_map(|back| Date::new(today.year.saturating_sub(back), month, day)).find(|d| *d <= today)
}

/// A name that starts at word `i`: one to [`MAX_NAME_WORDS`] words in capitals (not a month, a
/// day, or a word of the asking), or one address. With how many words it takes.
fn name_at(w: &[Word], used: &[bool], i: usize) -> Option<(String, usize)> {
    let first = w.get(i)?;
    if is_address(&first.text) && !used.get(i).copied().unwrap_or(true) {
        return Some((first.text.clone(), 1));
    }
    let mut parts = Vec::new();
    let mut j = i;
    while let Some(word) = w.get(j) {
        if parts.len() >= MAX_NAME_WORDS || !is_name_word(word) || used.get(j).copied().unwrap_or(true) {
            break;
        }
        parts.push(word.text.clone());
        j = j.saturating_add(1);
        if word.ends {
            break;
        }
    }
    (!parts.is_empty()).then(|| (parts.join(" "), parts.len()))
}

/// The name just before word `i` (which is `sent`): a run of words in capitals, back to the
/// first that isn't one. With the word it starts at.
fn name_before(w: &[Word], used: &[bool], i: usize) -> Option<(String, usize)> {
    let mut start = i;
    while let Some(p) = start.checked_sub(1) {
        let Some(word) = w.get(p) else { break };
        if i.saturating_sub(p) > MAX_NAME_WORDS || !is_name_word(word) || used.get(p).copied().unwrap_or(true) || word.ends {
            break;
        }
        start = p;
    }
    if start == i {
        return None;
    }
    let name: Vec<&str> = w.get(start..i)?.iter().map(|x| x.text.as_str()).collect();
    Some((name.join(" "), start))
}

/// Whether `word` may be part of a name: it starts with a capital, is letters (and `-`, `'`,
/// `.`), and isn't a month, a day of the week or a word of the asking.
fn is_name_word(word: &Word) -> bool {
    let l = word.lower.as_str();
    word.text.chars().next().is_some_and(char::is_uppercase)
        && word.text.chars().all(|c| c.is_alphabetic() || matches!(c, '-' | '\'' | '.'))
        && word.text.chars().count() <= 40
        && month(l).is_none()
        && weekday(l).is_none()
        && !NAME_STOPS.contains(&l)
        && !HEADS.contains(&l)
        && !LEAD.contains(&l)
        && !LINKS.contains(&l)
        && !matches!(l, "today" | "yesterday" | "not" | "except")
}

/// The one value `found` agrees on (by `key`), or none when it is empty or names two.
fn one<T: Clone, K: PartialEq>(found: &[T], key: impl Fn(&T) -> K) -> Option<T> {
    let first = found.first()?;
    found.iter().all(|f| key(f) == key(first)).then(|| first.clone())
}

/// Marks `len` words from `i` as explained.
fn mark(used: &mut [bool], i: usize, len: usize) {
    for k in i..i.saturating_add(len) {
        if let Some(u) = used.get_mut(k) {
            *u = true;
        }
    }
}

/// `key:value` of a term (`key` in small letters), if it is one: a key of two or more letters
/// and not a link (`http://`).
fn keyword(term: &str) -> Option<(String, &str)> {
    let (key, value) = term.split_once(':')?;
    (key.len() >= 2 && key.chars().all(|c| c.is_ascii_alphabetic()) && !value.starts_with("//")).then(|| (key.to_ascii_lowercase(), value))
}

/// The terms of a query as written: split on spaces outside double quotes, the quotes kept.
fn raw_terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in query.chars().take(super::narrow::MAX_PROPOSAL_CHARS.saturating_mul(2)) {
        if c.is_whitespace() && !quoted {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if quoted && c == '\\' && !escaped {
            escaped = true;
            cur.push(c);
            continue;
        }
        if c == '"' && !escaped {
            quoted = !quoted;
        }
        escaped = false;
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `terms` without a second copy of any (ignoring capitals).
fn dedup(terms: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in terms {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&t)) {
            out.push(t);
        }
    }
    out
}
