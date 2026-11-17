// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A small synthetic mailbox for retrieval's tests: invented people at `example.org` and
//! `.example`, invented text.

use mailcraft_model::{Address, Attachment, Body, FolderKind, Id, Mailbox, Message};

use crate::Session;

/// The messages of [`corpus`], by what each is.
#[allow(dead_code)]
pub(crate) struct Ids {
    /// From Harbor Hall: the deposit it asks for, with its floor plan attached.
    pub harbor: Id,
    /// About lunch at the harbour: a distractor for "harbor".
    pub lunch: Id,
    /// Ada's budget review, and Kit's reply in the same conversation.
    pub budget: Id,
    pub reply: Id,
    /// Ada about a cabin deposit: a distractor for "deposit".
    pub cabin: Id,
    /// A newsletter of common words.
    pub news: Id,
    /// A message in another folder (Projects).
    pub archived: Id,
    pub inbox: Id,
    pub archive: Id,
}

fn message(folder: Id, from: Address, subject: &str, body: &str, date: i64) -> Message {
    Message {
        folder,
        from,
        to: vec![Address::new("Robin Owner", "robin@example.org")],
        subject: subject.into(),
        date,
        body: Body { text: body.into(), html: None },
        ..Default::default()
    }
}

/// A session over the synthetic mailbox.
pub(crate) fn corpus() -> (Session, Ids) {
    let mut mb = Mailbox::with_local_account("Robin Owner", "robin@example.org");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).or_else(|| mb.folders.first().map(|f| f.id)).unwrap();
    let archive = mb.add_folder(account, None, "Projects").unwrap();
    let hall = Address::new("Harbor Hall Events", "events@harborhall.example");
    let ada = Address::new("Ada Byron", "ada@example.org");
    let kit = Address::new("Kit Tanaka", "kit@example.org");

    let mut harbor = message(
        inbox,
        hall,
        "Your booking at Harbor Hall",
        "Thank you for booking the main room for 12 March.\n\nWe ask for a deposit of 500 euros by Friday to hold the date.",
        1_000,
    );
    harbor.attachments = vec![Attachment::new("harbor-hall-floorplan.pdf", "application/pdf", vec![1, 2, 3])];
    let harbor = mb.add_message(harbor).unwrap();
    let lunch = mb.add_message(message(inbox, kit.clone(), "Lunch on Friday", "Shall we meet at the harbor cafe on Friday at noon?", 2_000)).unwrap();
    let mut budget =
        message(inbox, ada.clone(), "Quarterly budget review", "The budget for the fourth quarter is attached. Travel is over plan.", 3_000);
    budget.message_id = "budget-1@example.org".into();
    budget.attachments = vec![Attachment::new("q4-budget.xlsx", "application/vnd.ms-excel", vec![4])];
    let budget = mb.add_message(budget).unwrap();
    let mut reply = message(inbox, kit, "Re: Quarterly budget review", "Thanks, I will cut travel in the next quarter.", 4_000);
    reply.message_id = "budget-2@example.org".into();
    reply.in_reply_to = "budget-1@example.org".into();
    reply.references = vec!["budget-1@example.org".into()];
    let reply = mb.add_message(reply).unwrap();
    let cabin = mb.add_message(message(inbox, ada.clone(), "The cabin", "The cabin deposit was refunded to your card.", 5_000)).unwrap();
    let news = mb
        .add_message(message(
            inbox,
            Address::new("News", "news@lists.example"),
            "Weekly news",
            "This week in the city: the market, the park and the river.",
            6_000,
        ))
        .unwrap();
    let archived = mb.add_message(message(archive, ada, "Old notes", "Notes on last year's budget.", 500)).unwrap();
    let ids = Ids { harbor, lunch, budget, reply, cabin, news, archived, inbox, archive };
    (Session::new(mb), ids)
}
