#!/usr/bin/env python3
"""Builds the demo's data by running the REAL engine against a FICTIONAL machine.

Creates a throwaway home folder for an invented freelancer ("Dana"): a Claude
Code config with MCP servers, settings, agents, skills, and a month of
generated session logs across invented client folders. Then runs the core's
ignored live tests with HOME pointed at it and saves what they print. Nothing
here reads the real user's files, and every name is made up.

    python3 scripts/make-demo-fixture.py        # writes src/demo-fixture.json
"""
import json, os, random, shutil, subprocess, sys, tempfile, datetime, uuid, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
random.seed(20260921)

def seeded_uuid4():
    """A v4-shaped UUID drawn from the seeded `random` above, not from
    uuid.uuid4()'s own os.urandom source, which random.seed() can never
    reach. Every session/message id used to change on every run even with
    a frozen clock and a fixed seed -- and since those ids end up as
    HashMap keys on the Rust side, summing floats by iterating one back
    reads them in a different order each time, silently perturbing a cost
    total's last decimal place too. random.getrandbits() does read the
    seeded stream, so building a UUID's 128 bits from it instead makes
    ids, and every total added up by way of them, reproducible."""
    return uuid.UUID(int=random.getrandbits(128), version=4)

# The instant the whole synthetic month is generated relative to. A real
# datetime.now() here used to make every fixture:demo run rewrite this
# file's relative-day content (which weekday each synthetic day fell on
# changes which random.random() calls the "skip some weekends" check below
# consumes, which then reshuffles every later draw from the fixed seed
# above) even though nothing about the scenario changed. A fixed instant
# makes two consecutive runs byte-identical; bump it by hand (and re-run
# `npm run fixture:demo`) whenever the demo should look freshly generated
# again. Handed to the real engine as AITM_TODAY below, so its own "today"
# (the last-30-days window, the trend window) agrees with the day these
# logs were written relative to, instead of whatever day is real when this
# script happens to run.
FIXTURE_NOW = datetime.datetime(2026, 9, 25, 12, 0, 0, tzinfo=datetime.timezone.utc)
# realpath: macOS temp folders sit behind a symlink (/var -> /private/var),
# and the log walker will not follow symlinked paths.
home = pathlib.Path(os.path.realpath(tempfile.mkdtemp(prefix="aitm-demo-home-")))
work = home / "work"
claude = home / ".claude"

def compact(obj):
    """Claude Code writes compact JSON, and the engine's fast pre-check for
    assistant lines relies on it: no spaces after ':' or ','."""
    return json.dumps(obj, separators=(",", ":"))

def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)

# --- Claude Code config -----------------------------------------------------
write(home / ".claude.json", json.dumps({
    "mcpServers": {
        "context7": {"command": "npx", "args": ["-y", "@upstash/context7-mcp@1"]},
        "playwright": {"command": "npx", "args": ["-y", "@playwright/mcp@latest"]},
        "github": {"type": "http", "url": "https://api.githubcopilot.com/mcp/"},
        "postgres": {"command": "npx", "args": ["-y", "pg-readonly-mcp"], "env": {"DATABASE_URL": "demo-only"}},
    },
    "projects": {str(work): {"mcpServers": {"figma": {"command": "npx", "args": ["-y", "figma-context-mcp@2"], "env": {"FIGMA_KEY": "demo-only"}}}}},
}, indent=2))
write(claude / "settings.json", json.dumps({
    "model": "claude-sonnet-5",
    # None of these name Bash: the deny list keeps secrets and credentials out
    # of reach but leaves the shell itself unguarded, which is exactly the
    # gap the "deny-shell" audit check exists to catch.
    "permissions": {"allow": ["Bash(git status)", "Bash(npm test)"], "deny": ["Read(./.env)", "Write(./.env)", "Edit(./secrets/**)"]},
    "hooks": {"SessionEnd": [{"hooks": [{"type": "command", "command": "true"}]}]},
}, indent=2))
write(claude / "agents" / "deploy-checker.md", "---\nname: deploy-checker\ntools: Bash, Read\nmodel: sonnet\n---\nChecks a deploy before it ships.\n")
# No `tools:` and no `model:` line on purpose: this is the one agent in the
# demo that leaves both unset, so "agent-tools" and "agent-model" each have
# something to flag instead of sitting absent.
write(claude / "agents" / "release-notes.md", "---\nname: release-notes\n---\nDrafts release notes from merged PRs.\n")
for skill in ["deploy", "release-notes", "invoice"]:
    write(claude / "skills" / skill / "SKILL.md", f"---\nname: {skill}\n---\n")
for folder in ["acme-portal/web", "acme-portal/api", "northwind-api", "internal-tools", "blog"]:
    (work / folder).mkdir(parents=True, exist_ok=True)
write(home / "Library/Application Support/Claude/claude_desktop_config.json",
      json.dumps({"mcpServers": {"notes": {"command": "uvx", "args": ["notes-mcp"]}}}))
# The engine's own client rules, read by clients::load_from at clients::path()
# (HOME is this fictional home, so that resolves under it same as any other
# config file here). Same two clients, same patterns and budget src/demo/
# mock.ts's own clientRules hand-carries for the Clients tab -- keeping both
# in step is on the person editing either one, since nothing checks they
# still agree. Acme Co and Northwind are this demo's only fictional clients;
# never a real one.
write(home / "Library/Application Support/AITaskManager/clients.json", json.dumps({
    "rules": [
        {"client": "Acme Co", "patterns": ["acme-portal"], "monthlyBudget": 600},
        {"client": "Northwind", "patterns": ["northwind-api"]},
    ],
}))

# --- a month of session logs ------------------------------------------------
project_dir = claude / "projects" / "".join(c if c.isalnum() else "-" for c in str(work))
now = FIXTURE_NOW
AREAS = [("acme-portal/web", 0.34), ("acme-portal/api", 0.18), ("northwind-api", 0.24), ("internal-tools", 0.14), ("blog", 0.04), ("", 0.06)]
MODELS = [("claude-opus-5", 0.46, 1.9), ("claude-sonnet-5", 0.44, 0.5), ("claude-haiku-4-5-20251001", 0.10, 0.06)]

def pick(options):
    r, acc = random.random(), 0.0
    for name, weight, *rest in options:
        acc += weight
        if r <= acc:
            return (name, *rest)
    return (options[-1][0], *options[-1][2:])

def session(start, turns, sticky_area=None):
    sid = str(seeded_uuid4()); lines = []; t = start
    area = sticky_area if sticky_area is not None else pick(AREAS)[0]
    for i in range(turns):
        if i and random.random() < 0.08 and sticky_area is None:
            area = pick(AREAS)[0]
        model, unit = pick(MODELS)
        cost = round(unit * random.uniform(0.2, 1.6), 4)
        t += datetime.timedelta(minutes=random.uniform(1, 9))
        if t > now:
            break
        lines.append(compact({
            "type": "assistant", "timestamp": t.isoformat().replace("+00:00", "Z"), "sessionId": sid,
            "cwd": str(work / area) if area else str(work), "requestId": f"req_{seeded_uuid4().hex[:12]}", "costUSD": cost,
            "message": {"id": f"msg_{seeded_uuid4().hex[:16]}", "model": model, "content": [{"type": "text", "text": "."}],
                        "usage": {"input_tokens": random.randint(800, 9000), "output_tokens": random.randint(150, 2200),
                                  "cache_read_input_tokens": random.randint(20000, 160000)}},
        }))
    if lines:
        # The first line of a session is logged at the project root.
        first = json.loads(lines[0]); first["cwd"] = str(work); lines[0] = compact(first)
        write(project_dir / f"{sid}.jsonl", "\n".join(lines) + "\n")

for day in range(29, -1, -1):
    date = now - datetime.timedelta(days=day)
    if date.weekday() >= 5 and random.random() < 0.7:
        continue
    for _ in range(random.randint(1, 4)):
        start = date.replace(hour=random.randint(13, 21), minute=random.randint(0, 59))
        session(start, random.randint(6, 40))
# One session left open for weeks: the pattern the coaching is there to catch.
long_start = now - datetime.timedelta(days=19)
sid = str(seeded_uuid4()); lines = []; t = long_start
while t < now:
    t += datetime.timedelta(hours=random.uniform(5, 16))
    if t >= now: break
    lines.append(compact({"type": "assistant", "timestamp": t.isoformat().replace("+00:00", "Z"), "sessionId": sid,
        "cwd": str(work / "northwind-api"), "requestId": f"req_{seeded_uuid4().hex[:12]}", "costUSD": round(random.uniform(0.9, 3.4), 4),
        "message": {"id": f"msg_{seeded_uuid4().hex[:16]}", "model": "claude-opus-5", "content": [{"type": "text", "text": "."}],
                    "usage": {"input_tokens": 4000, "output_tokens": 900, "cache_read_input_tokens": 380000}}}))
first = json.loads(lines[0]); first["cwd"] = str(work); lines[0] = compact(first)
write(project_dir / f"{sid}.jsonl", "\n".join(lines) + "\n")

# One session with a subagent fan-out: deploy-checker (a real custom agent,
# so it stops reading "never run"), plus general-purpose and Explore (the
# built-ins Claude Code ships, never a Definition file). release-notes gets
# no subagent activity at all, so it stays the one unused custom agent.
# Area is northwind-api, same as the long-open session below: make-demo-
# fixture.py runs with AITM_AREA=northwind-api (see env, below), which is
# what live_sessions() uses for the fixture's own "sessions.area" -- and
# src/detail.ts's main Sessions list calls get_sessions with no area filter
# at all, which src/demo/mock.ts then serves from that same "area" array, so
# a session outside this one area would never actually show up there.
# deploy-checker's own fan-out is the one exception: its first run moves to
# an Acme Co area (below) so that agent's 30-day cost spans two clients,
# Acme in the majority -- the shape the Inventory tab's "mostly for {client}"
# clause needs at least one real row to show.
host_start = now - datetime.timedelta(days=2, hours=3)
host_sid = str(seeded_uuid4())
host_lines = []
t = host_start
for _ in range(5):
    t += datetime.timedelta(minutes=random.uniform(2, 6))
    host_lines.append(compact({
        "type": "assistant", "timestamp": t.isoformat().replace("+00:00", "Z"), "sessionId": host_sid,
        "cwd": str(work / "northwind-api"), "requestId": f"req_{seeded_uuid4().hex[:12]}", "costUSD": round(random.uniform(0.1, 0.6), 4),
        "message": {"id": f"msg_{seeded_uuid4().hex[:16]}", "model": "claude-sonnet-5", "content": [{"type": "text", "text": "."}],
                    "usage": {"input_tokens": random.randint(800, 4000), "output_tokens": random.randint(150, 900),
                              "cache_read_input_tokens": random.randint(10000, 60000)}},
    }))
first = json.loads(host_lines[0]); first["cwd"] = str(work); host_lines[0] = compact(first)
write(project_dir / f"{host_sid}.jsonl", "\n".join(host_lines) + "\n")

def subagent_transcript(agent_name, start, turns, model, area="northwind-api"):
    """A sidechain transcript stamped with attributionAgent, same line shape
    the real engine parses (crates/core/src/spend.rs's claude_line). Like
    every other log this script writes, the first line sits at the project
    root: claude_area's own "root" for a file is whatever cwd its first line
    carries, so without this every later line's identical cwd would compute
    as relative-to-itself (empty) instead of as `area`, and the whole file
    would book to (unsorted) no matter which folder it names."""
    lines = []
    t = start
    for _ in range(turns):
        t += datetime.timedelta(minutes=random.uniform(1, 4))
        lines.append(compact({
            "type": "assistant", "timestamp": t.isoformat().replace("+00:00", "Z"), "sessionId": host_sid,
            "cwd": str(work / area), "requestId": f"req_{seeded_uuid4().hex[:12]}", "costUSD": round(random.uniform(0.05, 0.4), 4),
            "isSidechain": True, "attributionAgent": agent_name,
            "message": {"id": f"msg_{seeded_uuid4().hex[:16]}", "model": model, "content": [{"type": "text", "text": "."}],
                        "usage": {"input_tokens": random.randint(500, 3000), "output_tokens": random.randint(100, 600),
                                  "cache_read_input_tokens": random.randint(5000, 40000)}},
        }))
    first = json.loads(lines[0]); first["cwd"] = str(work); lines[0] = compact(first)
    return "\n".join(lines) + "\n"

sub_dir = project_dir / host_sid / "subagents"
write(sub_dir / "deploy-checker-1.jsonl", subagent_transcript("deploy-checker", host_start + datetime.timedelta(minutes=10), 6, "claude-sonnet-5", area="acme-portal/web"))
write(sub_dir / "deploy-checker-2.jsonl", subagent_transcript("deploy-checker", now - datetime.timedelta(hours=6), 2, "claude-haiku-4-5-20251001"))
write(sub_dir / "general-purpose.jsonl", subagent_transcript("general-purpose", host_start + datetime.timedelta(minutes=20), 2, "claude-sonnet-5"))
write(sub_dir / "explore.jsonl", subagent_transcript("Explore", host_start + datetime.timedelta(minutes=30), 4, "claude-haiku-4-5-20251001"))

# --- run the real engine against it -----------------------------------------
env = dict(os.environ, HOME=str(home), CLAUDE_CONFIG_DIR="", XDG_CONFIG_HOME="", AITM_AREA="northwind-api",
           AITM_TODAY=FIXTURE_NOW.strftime("%Y-%m-%d"))
env.pop("CLAUDE_CONFIG_DIR"); env.pop("XDG_CONFIG_HOME")
env["PATH"] = f"{os.path.expanduser('~')}/.cargo/bin:" + env["PATH"]
env["CARGO_HOME"] = os.path.expanduser("~/.cargo"); env["RUSTUP_HOME"] = os.path.expanduser("~/.rustup")

FIXTURE_BEGIN, FIXTURE_END = "AITM_FIXTURE_BEGIN", "AITM_FIXTURE_END"

def run_ignored_test(test):
    """Runs one #[ignore]d fixture test under --nocapture and returns its stdout, split into lines."""
    out = subprocess.run(["cargo", "test", "-p", "aitm-core", "--lib", "--release", test, "--", "--ignored", "--nocapture"],
                         cwd=ROOT, env=env, capture_output=True, text=True).stdout
    return out.splitlines()

def live(test, marker):
    lines = run_ignored_test(test)
    for line in lines:
        if line.startswith(marker):
            return json.loads(line)
    # live_scan pretty-prints: its JSON starts on a line that is exactly "{".
    if "{" in lines:
        start = lines.index("{")
        end = len(lines) - 1 - lines[::-1].index("}")
        return json.loads("\n".join(lines[start:end + 1]))
    sys.exit(f"{test} printed no JSON. Last output:\n" + "\n".join(lines[-8:]))

def live_fixture(test):
    """Same idea as live(), but for a result that can legitimately be an
    empty array: a leading-substring marker like "[{" never matches an
    empty `[]`, so that heuristic misread a real empty result as no output
    at all. live_spend and live_agent_spend instead print their JSON
    between two sentinel lines that can never themselves be a JSON prefix,
    so an empty array is a valid result and a missing sentinel still fails
    loudly rather than guessing."""
    lines = run_ignored_test(test)
    if FIXTURE_BEGIN in lines and FIXTURE_END in lines:
        start, end = lines.index(FIXTURE_BEGIN) + 1, lines.index(FIXTURE_END)
        if start <= end:
            return json.loads("\n".join(lines[start:end]))
    sys.exit(f"{test} printed no {FIXTURE_BEGIN}/{FIXTURE_END} sentinel pair. Last output:\n" + "\n".join(lines[-8:]))

fixture = {
    "inventory": live("live_scan", "\x00"),
    "spend": live_fixture("live_spend"),
    "sessions": live("live_sessions", '{"area"'),
    "audit": live("live_audit", '{"generatedAt"'),
    "agentSpend": live_fixture("live_agent_spend"),
}
def round_floats(obj, ndigits=6):
    """The engine sums these by iterating a std HashMap, whose order is
    freshly randomized (SipHash keyed from OS entropy) every time the
    `cargo test` subprocess starts, even given byte-identical input and a
    frozen clock and seed -- so a cost total can land a few floating-point
    ULPs apart between two runs that agree on everything else, which is
    invisible to a person but not to a byte-for-byte diff. Rounding well
    past cent precision (every costUSD above was itself already rounded to
    4 places) collapses that jitter to one value both times, without
    reaching into the engine's own summation order to do it. A token count
    already arrives as a whole-numbered float and is unaffected; an int
    (a count, a score) fails the isinstance check below and passes through
    completely untouched."""
    if isinstance(obj, float):
        return round(obj, ndigits)
    if isinstance(obj, dict):
        return {k: round_floats(v, ndigits) for k, v in obj.items()}
    if isinstance(obj, list):
        return [round_floats(v, ndigits) for v in obj]
    return obj

# The fictional home's path must not leak a real one, and reads better short.
text = json.dumps(fixture).replace(str(home), "/Users/dana")
assert os.path.expanduser("~") not in text, "the real home directory leaked into the fixture"
write(ROOT / "src" / "demo-fixture.json", json.dumps(round_floats(json.loads(text)), indent=1))
shutil.rmtree(home, ignore_errors=True)
f = json.loads(text)
print("fixture written: %d MCP servers, %d tools, spend 30d $%.0f, %d areas, %d agent-spend rows, audit %s/100" % (
    len(f["inventory"]["mcpServers"]), len(f["inventory"]["tools"]), f["spend"][0]["last30"]["cost"],
    len(f["spend"][0]["projects"][0]["areas"]), len(f["agentSpend"]), f["audit"]["score"]))
