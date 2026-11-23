"""Exercise the installed stdio protocol with disposable synthetic mailbox files."""
import argparse
import json
from pathlib import Path
import secrets
import subprocess
import tempfile

CUTOFF = 10000


def invoke(cli, mailbox, key, methods, scope="1:11,12"):
    commands = [
        {"jsonrpc": "2.0", "id": i, "method": method, "params": params}
        for i, (method, params) in enumerate(methods, start=1)
    ]
    proc = subprocess.run([
        cli, "mcp", "--file", str(mailbox), "--header-scope", scope,
        "--header-cutoff", str(CUTOFF), "--header-key-file", str(key),
    ], input="".join(json.dumps(cmd) + "\n" for cmd in commands), text=True,
        capture_output=True, check=True, timeout=20)
    replies = [json.loads(line) for line in proc.stdout.splitlines()]
    assert [reply["id"] for reply in replies] == list(range(1, len(methods) + 1))
    return replies


def call(cli, mailbox, key, name, args, error=None, scope="1:11,12"):
    reply = invoke(cli, mailbox, key, [("tools/call", {"name": name, "arguments": args})], scope)[0]["result"]
    body = json.loads(reply["content"][0]["text"])
    if error is None:
        assert not reply["isError"], body
    else:
        assert reply["isError"] and body["code"] == error, body
    return body


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    binary = str(parser.parse_args().cli.resolve(strict=True))
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        mailbox = root / "pilot.mailcraft"
        key = root / "key"
        key.write_bytes(secrets.token_bytes(32))
        key.chmod(0o600)
        accounts = [{"id": i, "kind": "local", "name": f"Account {i}"} for i in (1, 2)]
        folders = [{"id": i * 10 + n, "account": i, "kind": k, "name": k.title()}
                   for i in (1, 2) for n, k in [(1, "inbox"), (2, "sent"), (3, "drafts")]]
        messages = {}
        def add(i, account, folder, subject, when, parent=None):
            msg = {"id": i, "account": account, "folder": folder, "subject": subject,
                   "date": when, "received": when, "messageId": f"{i}@example.invalid",
                   "body": {"text":"PRIVATE-FIXTURE-BODY"}, "hiddenText":"PRIVATE-HIDDEN-TEXT"}
            if parent:
                msg["references"] = [parent]
            messages[str(i)] = msg
        add(1, 1, 11, "Demo request", 1, "shared-parent")
        add(2, 1, 12, "Reply under a new subject", 2, "shared-parent")
        add(3, 1, 11, "Demo request", 3, "other-parent")
        add(4, 1, 11, "Unassigned", 4)
        add(5, 1, 13, "Out of scope draft", 5)
        add(6, 2, 21, "Out of scope other account", 6)
        add(7, 1, 12, "After cutoff", CUTOFF + 1, "shared-parent")
        add(8, 1, 12, "Automatic notice", 8, "shared-parent")
        messages["8"]["headers"] = [["Auto-Submitted", "auto-replied"]]
        native = {"accounts": accounts, "folders": folders, "messages": messages}
        original = json.dumps(native).encode()
        mailbox.write_bytes(original)
        query = {"account":1, "folders":[11,12], "cutoff":CUTOFF, "fields":["subject"]}
        page = "page_message_headers"
        resolve = "resolve_message_evidence"
        def tool(name, args, error=None, scope="1:11,12"):
            return call(binary, mailbox, key, name, args, error, scope)

        listing, resources, denied_resource = invoke(binary, mailbox, key, [
            ("tools/list", {}), ("resources/list", {}),
            ("resources/read", {"uri":"mailcraft://mailbox"}),
        ])
        assert {t["name"] for t in listing["result"]["tools"]} == {page, resolve}
        assert resources["result"]["resources"] == [] and "error" in denied_resource
        tool("send_message", query, "invalid_arguments")
        first = tool(page, dict(query, limit=2))
        rest = tool(page, dict(query, cursor=first["nextCursor"], limit=100))
        rows = first["rows"] + rest["rows"]
        assert len(rows) == 5 and rest["nextCursor"] is None
        assert [r["timestamp"] for r in rows] == [1,2,3,4,8]
        assert rows[0]["threadRef"] == rows[1]["threadRef"] != rows[2]["threadRef"]
        assert rows[3]["threadRef"] is None and rows[4]["direction"] == "auto"
        assert not any("PRIVATE" in json.dumps(r) for r in rows)
        assert tool(resolve,dict(query,sourceRef=rows[0]["sourceRef"]))["row"] == rows[0]
        assert tool(page,dict(query,folders=[11]))["coverage"] == {"inbox": True,"sent": False}
        assert all("subject" not in r for r in tool(page,dict(query,fields=[]))["rows"])
        tool(page, dict(query, account=2), "scope_denied")
        tool(page, dict(query, folders=[11,21]), "scope_denied")
        tool(page, dict(query, cutoff=CUTOFF+1), "scope_denied")
        tool(page, dict(query, limit=101), "invalid_limit")
        tool(page, dict(query, fields=["body"]), "unsupported_projection")
        tool(page, dict(query, cursor="bad"), "invalid_cursor")
        tool(page, dict(query, cursor="0"*64), "stale_cursor")
        tool(page, dict(query, folders=[11], cursor=first["nextCursor"]), "stale_cursor")
        tool(page, dict(query, fields=[], cursor=first["nextCursor"]), "stale_cursor")
        tool(resolve, dict(query, sourceRef="f"*64), "invalid_reference")
        tool(resolve, dict(query, folders=[11], sourceRef=rows[0]["sourceRef"]), "invalid_reference", scope="1:11")
        assert mailbox.read_bytes() == original and set(root.iterdir()) == {key,mailbox}
        messages["1"]["subject"] = "Changed"
        mailbox.write_text(json.dumps(native))
        tool(page, dict(query,cursor=first["nextCursor"]), "stale_cursor")
        tool(resolve, dict(query,sourceRef=rows[0]["sourceRef"]), "invalid_reference")
    print("Scoped MailCraft stdio paging, independent resolution, denials, freshness and write observations passed.")


if __name__ == "__main__":
    main()
