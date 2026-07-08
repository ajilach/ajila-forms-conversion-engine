# Pipeline lessons — cross-cutting gotchas

Hard-won operational lessons that aren't tied to a single sweep. Read this before
running or extending the pipeline — every item here cost real debugging time at
least once. Per-problem knowledge lives in [`consistent-problems.md`](consistent-problems.md);
architecture lives in [`../../docs/design.md`](../../docs/design.md); the operating
procedure lives in the `/sweep` and `/feedback` skills. This file is the "things
that bite you" layer on top of those.

---

## 1. The engine is ground truth — but run it *fresh*, and know when to diverge

The conversion engine (`ajila-forms-conversion-engine`) is the reference for "what
should the AEM look like." When deciding what a sweep should produce:

- **Run a fresh conversion**, don't trust a cached/previous run — the engine and its
  profile templates (`profiles/ubs/aem/*.xml`) change. A stale conversion has burned
  us into "fixing" toward an old shape.
- **Check the template, not just the output.** For structure questions the profile
  templates (`fragment.xml`, `panel.xml`, `root.xml`, …) are often faster and more
  authoritative than a full conversion.
- **The engine is fallible.** Some sweeps deliberately *diverge* from it because the
  engine is wrong or incomplete. When you diverge, say so in the registry entry's
  `Origin:` (mark it a *candidate engine-fix*) and record *why*. Examples:
  - jump-to-field (#22): engine emits `jumpToFieldButtonVisible` on the title *draw*,
    where it has no effect — the sweep puts it on the *panel* (upstream bug).
  - banking-relationship-wrap (#29): engine sets `summaryExclusion`/`dorExclusion`
    *on* the fragment panel; the sweep wraps the subtree in a `PN_BR` panel instead,
    because flags on the fragment panel alone may not exclude the rendered subtree
    from the summary.

## 2. Git LFS ZIPs do not 3-way-merge — sequence overlapping rollouts

`forms/issued/<form>/<form>_merged.zip` is a binary tracked via Git LFS. Two sweep
branches that both edit the same form's ZIP **conflict at merge** — there is no
content-level 3-way merge. Consequences:

- **Sequence overlapping sweeps.** Fully merge one sweep, then `git pull` master,
  rebase the next sweep's work on the new base, and `--continue`. Because every
  `set_*.py` re-derives the fixed ZIP from the base each run, and detectors skip
  already-fixed forms (see §3), re-running on the merged base is safe.
- **A stale sweep branch is a trap.** If another sweep merged after you branched,
  your branch's ZIPs are stale — don't merge it. Delete the branch and re-pilot
  fresh on the new master (this happened between #24 and #22 → PR #23 was dropped,
  re-piloted as #26).
- `run_sweep.py` never switches the main checkout to the sweep branch — it commits
  fixed ZIPs via a throwaway worktree and restores `forms/issued` at the end. Don't
  defeat this by hand-checking-out the sweep branch in the main tree.

## 3. Detectors must be idempotent — "already fixed" is a skip, not a match

Because rollouts get rebased and re-run (§2), a detector **must not** re-report a
form it already fixed. Every `find_*.py` needs an explicit "is this already in the
target shape?" guard that returns *not-affected*. Verify idempotency before rollout:
run the fixer twice on one form — the second run must be a no-op (`wrapped:0`,
`swapped:0`, etc.). A detector that keys only on the *problem* signature and not the
*fixed* signature will loop forever on `--continue`.

## 4. The tracking-issue table is machine-managed — never hand-edit it

`run_sweep.py` / `sweep_issue.py` write a `<!-- sweep-table -->` region into the
GitHub issue body (idempotent per-form upsert). **`gh issue edit --body-file` of the
whole body CLOBBERS that region** — this has wiped the table twice. Rules:

- Don't edit the issue body (even the human-written description) *while a sweep is
  running* — your edit and the sweep's table write race, and the last writer wins.
- To change the description, edit only outside the managed region, or rebuild the
  table afterward with `sweep_issue.replace_managed`.

## 5. Structural edits re-path pre-existing validation errors → benign `delta` flags

The `delta` gate compares MCP validation before/after by **full node path string**.
Any sweep that **moves or wraps** a node (wrap, promote, reorder) changes the paths
of everything beneath it. If the moved subtree already had a pre-existing violation
(e.g. an `items is missing sling:resourceType` that shipped on master), the old path
shows as "resolved" and the new deeper path shows as "new" — a **phantom** flag with
**zero net change** in the violation count.

- Before treating a `delta` flag as real, diff the before/after violation *sets* and
  check the net count. One-in / one-out at a re-pathed location = benign.
- This is expected for wrap/move/reorder sweeps; note it in the registry review note
  so reviewers don't chase it. (First seen: banking-relationship-wrap #29, AAAE.)

## 6. "First page only" and other step-scoped gates

Several sweeps must act only on a specific wizard step (usually the first page).
A *step* = a panel directly under `guideContainer › rootPanel(guideRootPanel) ›
items`, excluding specials (`summary`/`preview`/`formmetadata`/`signerInfo`/…). The
first real such panel is page 1. Recurring traps:

- Name-based detection over-matches — e.g. a `PN_FormSection`/`PN_FormConfigurator`
  can appear on a *later* step and get wrongly swept. Gate on **step index 0**, not
  name alone (jump-to-field #22 dropped ~5 false matches this way).
- Always **report the on-page vs off-page breakdown** in the pilot so the owner can
  confirm the gate against real numbers before `--continue`.

## 7. Byte-offset XML surgery, not a DOM rewrite

Form `.content.xml` files carry rich-text `_value` attributes containing literal
`>` inside `&lt;p>…`, and the exact byte layout matters for LFS diffs. So the
`set_*.py` fixers edit by **byte offset** (quote-aware open-tag scan + depth-scan
element spans via `expat`), not by re-serializing a DOM. Practical rules:

- **Move/wrap nodes byte-for-byte** — copying the element's exact bytes preserves
  `fragRef`, `bindRef`, `name`, and translation linkage. Never re-emit from parsed
  attrs (you'll drop or reorder something).
- **Splice multiple edits back-to-front** so earlier byte offsets stay valid.
- **Always re-parse the result with `expat`** and return a `ok`/well-formed boolean;
  `run_sweep`'s verify gates on it.
- Boolean attrs use the JCR `{Boolean}true` form, dates `{Date}…` — match existing
  values verbatim.
- `rm -rf _edit` before regenerating a staged file; patch in one pass (don't layer
  edits across runs on the same staged tree).

## 8. AEM state: JCR JSON reads, `.env` writes

- **Reading deployed state:** the render HTML endpoint returns 401, but the JCR JSON
  does not — `curl -u admin:admin http://localhost:4502/…/jcr:content/….infinity.json`.
- **Deploying:** `aem_install.py` using creds from **`.env`** — *not* the MCP
  `upload_aem_package_from_file` (that reads the desktop app's `history.db`, which the
  pipeline doesn't set). The `blueprint` MCP is used **only** for
  `validate_aem_package_from_file`.

## 9. Run a full `--continue` in the background, as one run

Don't split a rollout into foreground `--pilot N` chunks — foreground has a ~10-min
cap. Run the full `--continue` **once, in the background**; `run_sweep.py` prints
per-form progress (`… working` → `✓ done`/`⚠ FLAGGED` + a running tally) that you can
tail. The staged **pilot → human review → `--continue`** gate still stands: only
`--continue` after the pilot previews have been eyeballed.

## 10. Swept problems are guarded in CI — keep the baseline green

`.github/workflows/sweep-regression.yml` runs `check_regressions.py`, which re-runs
the `Detect` verb of **every problem whose registry Status says "swept"** and fails if
any form is affected. On a PR it checks only the forms the PR changed; on push to
master it checks the whole corpus. This is what stops a newly-authored/edited form
from silently re-introducing a defect a sweep already fixed.

- **Enrollment is automatic** — setting `**Status:** swept` is all it takes; the guard
  discovers it from the registry. No per-sweep CI wiring.
- **A half-wired swept entry now fails the guard** — if a swept entry's `Detect` verb can't
  be resolved (typo, unknown verb), `check_regressions.py` exits non-zero instead of silently
  skipping it. A swept problem that can't be detected would be silently unenforced, so it's
  treated as a registry bug. Keep every swept entry's `Detect` a valid `grep:`/`script:` verb.
- **A sweep is only closed out when its detector reports 0 affected corpus-wide** —
  run `python3 .claude/scripts/check_regressions.py` at closeout. A red baseline means
  the rollout missed forms or the detector isn't idempotent (§3), and it will turn the
  master backstop red for everyone.
- Detectors are pure Python + stdlib, so the guard needs no AEM and no engine binary —
  just an LFS checkout.

## 11. A detector must flag a form unless it is FULLY correct — check values, not just structure

The regression guard reuses each sweep's detector, so **"not affected" must mean "in the
correct target state," including property *values*** — not merely "the structure is
present." A detector that recognizes the fix by shape alone lets a form with the right
structure but a wrong/missing property value pass as clean.

- Concrete miss (fixed): `find_banking_wrap.py` (#29) treated any enclosing panel named
  `PN_BR` as wrapped. A `PN_BR` that had lost `summaryExclusion="true"` (or
  `dorExclusion="true"`) still counted as clean. Fix: `is_wrapped` now requires the panel
  to actually carry **both** exclusion flags (UBS panel, sole child) — the name alone is
  not sufficient.
- Good pattern to copy: `find_jump_to_field.py` computes a **delta per attribute**
  (`attr != "true"` → flag), so every value is asserted; `find_panel_noncanonical.py`
  (#10) takes `--require-attr dorExclusion=true`. Value-based sweeps (#8/#10/#14/#20/#24/#27)
  and #22 already satisfy this; a purely structural target (#18 — "title wrapped, shown
  once") has no separate value to check.
- Rule of thumb when writing a new detector: for each property the fix *sets*, the
  detector must treat "present but wrong value" the same as "absent." Test it: take a
  correctly-fixed form, corrupt one swept property value, and confirm the detector flags
  it (not just when the node is missing).

## 12. Repair reuses the sweep — don't re-diagnose what a sweep already fixes

The registry is the single source of truth, and both halves of the loop derive from it:
`check_regressions.py` **detects** every swept invariant a form violates; `apply_sweeps.py`
**repairs** them (each swept entry's `script:` fix, in an idempotent fixpoint). Anything that
brings a form in should *reuse* those, not hand-roll a fix.

- **`/feedback` fixes swept problems via the sweep first.** The Worker runs
  `apply_sweeps.py --forms <FORM>` before manual diagnosis; items matching a swept entry are
  resolved deterministically (consistent with every other form), and only the rest are
  hand-fixed. Same fix, one place.
- **Stale feedback is common — verify before fixing.** QA feedback is point-in-time; the form
  may already have been fixed (by a sweep in a later PR, an earlier feedback PR, or a
  re-conversion). If `apply_sweeps` changes nothing for a matched swept problem, or an item no
  longer reproduces, close it as *already resolved* — don't force a no-op edit to have a diff.
- **The intake app never runs stale sweep logic.** It creates each intake/bulk worktree from
  *freshly-fetched* `origin/master` and runs that worktree's detectors/fixers/registry — so
  sweep coverage is tied to master, not to how current the local checkout is. (The app's own
  UI code can lag; it flags that separately.)
- **Reach for `apply_sweeps.py` whenever a form fails the guard** — `--forms`, `--affected`
  (every currently-violating form), or `--all`. It's the repair counterpart to
  `check_regressions.py`; re-run the guard after to confirm green.
