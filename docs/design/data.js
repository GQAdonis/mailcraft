/* Example mailbox for the MailCraft proposal prototype. All people, companies and addresses are fictional. */
window.MC_DATA = {
  today: "2026-10-14",
  owner: { name: "Avery Quinn", addr: "avery@lumen.example" },

  accounts: [
    { id: "work", addr: "avery@lumen.example", label: "Work (Microsoft 365)" },
    { id: "home", addr: "avery@quinnfamily.example", label: "Personal (IMAP)" }
  ],

  folders: [
    { id: "inbox", name: "Inbox", acct: "work" },
    { id: "drafts", name: "Drafts", acct: "work", quietCount: true },
    { id: "sent", name: "Sent Items", acct: "work" },
    { id: "archive", name: "Archive", acct: "work" },
    { id: "junk", name: "Junk Email", acct: "work" },
    { id: "home-inbox", name: "Inbox", acct: "home" }
  ],

  messages: [
    { id: "m1", folder: "inbox", group: "Pinned", pinned: true, from: "Priya Nair", addr: "priya@lumen.example", to: "Atlas team", subject: "Welcome to the Atlas project", time: "Sep 20", date: "2026-09-20", unread: false, flagged: false,
      body: ["Team, Atlas is go. The short version: one shared plan, one release train, and a weekly demo on Thursdays.", "The project page has the roles, the calendar and the decision log. If something isn't written there, it hasn't been decided yet.", "Priya"] },
    { id: "m2", folder: "inbox", group: "Today", from: "Maya Okafor", addr: "maya@lumen.example", to: "Avery Quinn; Tomás Rivera", subject: "Homepage mockups, round three", time: "12:42 PM", date: "2026-10-14", unread: false, flagged: false, attach: { name: "homepage-round-3-notes.txt", size: "224 bytes" }, category: "Design",
      body: ["Hi both,", "Round three is attached. The big change is the hero: one message, one button, and the customer quotes move below the fold.", "Two things I'd like your eyes on: the pricing teaser (is three tiers too much for the first screen?) and the dark variant. I think it reads better; Tomás thinks it hides the illustration.", "Notes for each screen are in the text file. If we can agree by Thursday I can hand off to build on Friday.", "Maya"] },
    { id: "m3", folder: "inbox", group: "Today", from: "Priya Nair", addr: "priya@lumen.example", to: "Avery Quinn", subject: "Q4 roadmap review moved to Thursday", time: "12:25 PM", date: "2026-10-14", unread: true, flagged: false, important: true,
      body: ["Avery,", "The roadmap review is moving to Thursday at 2:00 PM in Room Oak, because finance can't make Wednesday.", "Please bring the three options for the Atlas launch date, with the risk you see in each. Fifteen minutes is enough.", "Priya"] },
    { id: "m4", folder: "inbox", group: "Today", from: "Cloudline Billing", addr: "billing@cloudline.example", to: "Avery Quinn", subject: "Your October invoice is ready", time: "12:10 PM", date: "2026-10-14", unread: true, flagged: false, attach: { name: "INV-20481.pdf", size: "88 KB" },
      body: ["Your October invoice INV-20481 is ready.", "Amount due: $1,240.00 for 12 seats of Cloudline Team, due on October 31, 2026.", "The invoice is attached as a PDF. Payment runs automatically from the card ending 4417 unless you change it before the due date."] },
    { id: "m5", folder: "inbox", group: "Yesterday", from: "Tomás Rivera", addr: "tomas@lumen.example", to: "Avery Quinn", subject: "RE: Release checklist", time: "Tue 4:20 PM", date: "2026-10-13", unread: false, flagged: true, due: "Due Friday",
      body: ["Good catch on the installer signing step. I've added it to the checklist as step 7, before the upload.", "One open item: we still don't have the new signing certificate. Jonas says it should arrive this week. If it doesn't, the release slips to the following Tuesday.", "Tomás"] },
    { id: "m6", folder: "inbox", group: "Yesterday", from: "Jonas Weber", addr: "jonas@lumen.example", to: "Everyone at Lumen", subject: "Expense reports are due Friday", time: "Tue 11:05 AM", date: "2026-10-13", unread: false, flagged: false,
      body: ["A reminder that September expense reports are due this Friday, October 16.", "Receipts over $50 need a photo. Anything late goes into the November run.", "Jonas"] },
    { id: "m7", folder: "inbox", group: "Monday", from: "Lin Zhao", addr: "lin@lumen.example", to: "Avery Quinn", subject: "Feedback on the onboarding flow", time: "Mon 2:48 PM", date: "2026-10-12", unread: true, flagged: false,
      body: ["Hello Avery,", "We rolled the new onboarding flow out to 10% of sign-ups last week. The second step, where people connect their calendar, is where most of them stop.", "Could we look at it together this week? I have Thursday morning free.", "Lin"] },
    { id: "m8", folder: "inbox", group: "Last week", from: "Cloudline Billing", addr: "billing@cloudline.example", to: "Avery Quinn", subject: "Your September invoice is ready", time: "Oct 2", date: "2026-10-02", unread: false, flagged: false, attach: { name: "INV-19877.pdf", size: "86 KB" },
      body: ["Your September invoice INV-19877 is ready.", "Amount due: $1,140.00 for 11 seats of Cloudline Team, paid on September 30, 2026."] },
    { id: "m9", folder: "drafts", group: "Today", from: "Avery Quinn", addr: "avery@lumen.example", to: "Lin Zhao", subject: "RE: Feedback on the onboarding flow", time: "9:02 AM", date: "2026-10-14", unread: false, flagged: false, draft: true,
      body: ["Thursday morning works. Let's"] },
    { id: "m10", folder: "junk", group: "Today", from: "Prize Desk", addr: "win@prize-desk.example", to: "Avery Quinn", subject: "You have been selected", time: "7:14 AM", date: "2026-10-14", unread: true, flagged: false,
      body: ["Claim your reward today."] },
    { id: "m11", folder: "home-inbox", group: "Yesterday", from: "Sam Quinn", addr: "sam@quinnfamily.example", to: "Avery Quinn", subject: "Dinner on Saturday?", time: "Tue 8:10 PM", date: "2026-10-13", unread: true, flagged: false,
      body: ["Are we still on for Saturday? I can bring dessert.", "Sam"] }
  ],

  avatarColors: ["#0B7FAB", "#2E8B57", "#B35C1E", "#7E57C2", "#B03A5B", "#3F6FB5", "#6B7A1F", "#8A5A44"],

  /* Scripted answers for the prototype. Real answers come from the owner's model; citations point at real message ids. */
  answers: [
    { match: ["release", "signing", "certificate", "checklist", "slip"],
      title: "Is the release still on track?",
      parts: [
        "Not yet certain. The release depends on the new signing certificate, which hasn't arrived {m5}.",
        "Tomás added the installer signing step to the checklist as step 7 {m5}. If the certificate doesn't come this week, the release moves to the following Tuesday {m5}.",
        "Nothing in your mail says the certificate has arrived. The follow-up on Tomás's message is due Friday."
      ], read: 6 },
    { match: ["roadmap", "thursday", "review", "meeting", "this week", "prepare", "bring"],
      title: "What do I need to prepare for Thursday?",
      parts: [
        "Two things are on Thursday.",
        "• The Q4 roadmap review at 2:00 PM in Room Oak. Priya wants the three options for the Atlas launch date, with the risk of each, in about fifteen minutes {m3}.",
        "• Maya wants agreement on the homepage mockups by Thursday, so she can hand off to build on Friday: the pricing teaser and the dark variant are the open questions {m2}.",
        "Lin also offered Thursday morning to look at the onboarding drop-off together, and you haven't answered yet {m7}."
      ], read: 8 },
    { match: ["invoice", "cloudline", "billing", "owe", "pay", "cost", "seat"],
      title: "What do we pay Cloudline?",
      parts: [
        "October's invoice is $1,240.00 for 12 seats, due October 31 and paid automatically from the card ending 4417 {m4}.",
        "That's $100 more than September, which was $1,140.00 for 11 seats {m8}. The difference is one added seat."
      ], read: 4 },
    { match: [],
      title: "",
      parts: [
        "Here's what looks like it needs you this week.",
        "• Priya moved the Q4 roadmap review to Thursday at 2:00 PM and wants three launch-date options {m3}.",
        "• Tomás's release checklist is waiting on a signing certificate; that follow-up is due Friday {m5}.",
        "• Lin asked to review the onboarding drop-off together, and your reply is still a draft {m7}.",
        "I only found these in Work's Inbox. Mail from your personal account wasn't searched."
      ], read: 11 }
  ],

  suggestedQuestions: [
    "What do I need to prepare for Thursday?",
    "Is the release still on track?",
    "What do we pay Cloudline?"
  ],

  /* Plain-words narrowing: phrase → query parts (the model proposes; the parser validates) */
  narrowRules: [
    { re: /\b(invoice|invoices|bill|bills)\b/i, part: "invoice" },
    { re: /\bcloudline\b/i, part: "from:cloudline" },
    { re: /\bmaya\b/i, part: "from:maya" },
    { re: /\bpriya\b/i, part: "from:priya" },
    { re: /\btom[aá]s\b/i, part: "from:tomas" },
    { re: /\bjonas\b/i, part: "from:jonas" },
    { re: /\blin\b/i, part: "from:lin" },
    { re: /\blast month\b/i, part: "after:2026-09-01" },
    { re: /\bthis week\b/i, part: "after:2026-10-12" },
    { re: /\b(attachment|attachments|attached|pdf|file)\b/i, part: "has:attachment" },
    { re: /\bunread\b|haven'?t read/i, part: "is:unread" },
    { re: /\bflagged\b|follow.?up/i, part: "is:flagged" },
    { re: /\b(mockup|mockups|homepage)\b/i, part: "mockups" },
    { re: /\b(release|checklist)\b/i, part: "release" },
    { re: /\broadmap\b/i, part: "roadmap" }
  ],

  agents: {
    connected: [
      { id: "claude", name: "Claude", via: "MCP · connected 9:12 AM", reads: true },
      { id: "codex", name: "Codex", via: "MCP · connected yesterday", reads: true }
    ],
    pending: [
      { id: "r1", agent: "Claude", kind: "send", what: "wants to send a message for you",
        preview: { To: "Lin Zhao <lin@lumen.example>", Subject: "RE: Feedback on the onboarding flow", body: "Hi Lin,\nThursday morning works for me. Shall we say 10:00 in Room Birch? I'll bring the drop-off numbers for step two.\nAvery" } },
      { id: "r2", agent: "Codex", kind: "delete", what: "wants to delete 1 message in Junk Email",
        preview: { From: "Prize Desk <win@prize-desk.example>", Subject: "You have been selected" }, targets: ["m10"] }
    ],
    log: [
      { dot: "ok", text: "Claude searched mail for “onboarding drop-off” (3 results)", time: "10:41 AM" },
      { dot: "ok", text: "Claude read 3 messages from Lin Zhao", time: "10:41 AM" },
      { dot: "ask", text: "Claude asked to send a reply to Lin Zhao", time: "10:42 AM" },
      { dot: "ok", text: "Codex listed Junk Email", time: "10:50 AM" },
      { dot: "ask", text: "Codex asked to delete 1 message", time: "10:50 AM" }
    ]
  },

  suggestedReply: {
    m2: "Hi Maya,\n\nThanks, round three is a clear step forward. One message and one button in the hero works.\n\nOn the pricing teaser: I'd show two tiers on the first screen and link to the full table. On the dark variant, I'm with Tomás that the illustration gets lost; could we try it with a lighter illustration background?\n\nHappy to agree the rest on Thursday so you can hand off on Friday.\n\nAvery",
    m3: "Hi Priya,\n\nThursday at 2:00 in Room Oak works. I'll bring three options for the Atlas launch date, each with its main risk, and keep it to fifteen minutes.\n\nAvery",
    m5: "Thanks, Tomás. Step 7 looks right. I'll check with Jonas on the certificate tomorrow and let you know by Thursday whether we're holding the date.\n\nAvery",
    m7: "Hi Lin,\n\nThursday morning works. Shall we say 10:00? I'll look at the step-two numbers beforehand.\n\nAvery",
    fallback: "Hi,\n\nThanks for this. I'll get back to you by the end of the week.\n\nAvery"
  }
};
