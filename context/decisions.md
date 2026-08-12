# Decisions

Record of every deliberate deviation from the plan (`build-plan.md` / `progress-tracker.md`). The plan documents how things are *supposed* to be built; this file documents when reality forced or justified something different — and why.

Without this file, a future session sees the plan, assumes it was followed, and re-introduces inconsistency. Read this file before starting any feature.

---

## How to Use

**Before starting a feature:** check for entries tagged with that step number. If one exists, it overrides what the plan says.

**When deviating:** the moment implementation would do something the plan doesn't say (or says differently), write the entry *at that time* — not at the end, not "later", and never silently. A deviation is only a problem if it happens off the record.

**What counts as a deviation:**

- A locked (bold) choice in progress-tracker.md that we didn't follow
- A format/behavior choice where the plan didn't specify and real git is ambiguous
- A deliberate scope cut (e.g., "not implementing X in v1")
- An added dependency, command, or feature not in the plan

**What does not** belong here: routine bug fixes, refactors that don't change behavior, or anything already specified in the plan.

---

## Entry Template

Copy this for each new decision. Number sequentially (`D-001`, `D-002`, ...). Append at the end of the log — never rewrite history.

```markdown
## D-00X — <Short title>

- **Date:** YYYY-MM-DD
- **Step(s) affected:** <step numbers from progress-tracker.md, or "global">
- **Plan said:** <what the plan/progress-tracker specifies>
- **Decision:** <what we actually did, precisely>
- **Why:** <the concrete reason — a real git behavior found, a constraint, a time-box>
- **Impact:** <what downstream work depends on this, what would break if reversed>
```

---

## Decision Log

*(No decisions yet — all deviations go here from now on.)*