"""
Run the feedback repo's regression guard on a package this engine just converted.

The sweeps in `../ajila-forms-conversion-feedback` fix systemic defects across the
deployed corpus, and its CI guard (`.claude/scripts/check_regressions.py`) fails any
form that re-introduces one. A form this engine converts is such a form: it goes into
that corpus. So the guard is the acceptance test for the AEM output, and this script is
how to run it without importing the form into the corpus first.

How it works: convert the PDFs with `blueprint-cli --aem`, then build the same one-form
harness the feedback repo's intake app builds (`scripts/intake/core.py::_check_harness`).
Every detector derives its repo root from `os.path.abspath(__file__)`, which normalises
lexically and does NOT resolve symlinks, so a directory that merely looks like the repo
IS the repo as far as they are concerned:

    <harness>/.claude                        -> symlink to the feedback repo's scripts
    <harness>/feedback/knowledge             -> symlink to the registry
    <harness>/forms/issued/<F>/<F>_merged.zip = the package the CLI just wrote
    <harness>/forms/issued/<F>/<F>_<LANG>.pdf = the source PDFs (a detector may read them)

Each detector's `glob("forms/issued/*/*_merged.zip")` then matches exactly the forms
under test instead of the ~304 of the corpus.

`--no-skip` is passed on purpose: the registry's Skip lines name forms the corpus has
not got round to repairing, which is no excuse for a package being produced now.

Usage:
    python3 scripts/check_feedback_rules.py core/input/AAOS_033_IT.pdf core/input/AAOV_033_IT.pdf
    python3 scripts/check_feedback_rules.py core/input/BAGE_019_{DE,EN}.pdf   # one form, two languages
    python3 scripts/check_feedback_rules.py --json ... > report.json
    python3 scripts/check_feedback_rules.py --keep ...      # keep the scratch dir and say where

PDFs are grouped into forms by the `<CODE>_<ENTITY>` prefix of their file name, so
several forms (and several languages per form) can be checked in one run.

Exit code: 0 when the guard reports no violation, 1 otherwise (2 on a setup or
conversion failure), so this can gate a commit.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

ENGINE_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_FEEDBACK_REPO = os.path.join(os.path.dirname(ENGINE_ROOT), "ajila-forms-conversion-feedback")

# `AAOS_033_IT.pdf` -> form `AAOS_033`, language `IT`. The language suffix is optional
# (`AAOS_033.pdf`); the form part is what groups the PDFs.
PDF_NAME = re.compile(r"^(?P<form>[A-Z][A-Za-z0-9]*_\d{3})(?:_(?P<lang>[A-Za-z-]+))?$")


def group_by_form(pdf_paths):
    """{form_code: [(abs pdf path, LANG or None)]}, in the order given."""
    forms = {}
    for p in pdf_paths:
        ap = os.path.abspath(p)
        if not os.path.isfile(ap):
            die(f"no such file: {p}")
        m = PDF_NAME.match(os.path.splitext(os.path.basename(ap))[0])
        if not m:
            die(f"cannot read a form code out of {os.path.basename(ap)} "
                f"(expected <CODE>_<ENTITY>[_<LANG>].pdf, e.g. AAOS_033_IT.pdf)")
        forms.setdefault(m.group("form"), []).append((ap, m.group("lang")))
    return forms


def die(msg, code=2):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(code)


def convert(pdfs, out_dir, profile, cli):
    """Run the CLI on one form's PDFs and return the package it wrote."""
    cmd = cli + [p for p, _ in pdfs] + ["--aem", "--profile", profile]
    r = subprocess.run(cmd, cwd=out_dir, capture_output=True, text=True)
    zips = [f for f in os.listdir(out_dir) if f.endswith(".zip")]
    if r.returncode != 0 or not zips:
        sys.stderr.write(r.stdout[-4000:])
        sys.stderr.write(r.stderr[-4000:])
        die(f"conversion failed for {os.path.basename(pdfs[0][0])}")
    if len(zips) > 1:
        die(f"conversion wrote more than one package: {sorted(zips)}")
    return os.path.join(out_dir, zips[0])


def build_harness(root, packages, feedback_repo):
    """A directory the feedback detectors read as their repo, holding only `packages`.

    `packages` is {form_code: (zip path, [(pdf path, LANG)])}. The zip is COPIED (the
    scratch conversion dir is temporary), the PDFs are copied too because at least one
    detector (PROBLEM-metadata-languages) derives the form's languages from the file
    names next to the package.
    """
    os.makedirs(root, exist_ok=True)
    os.symlink(os.path.join(feedback_repo, ".claude"), os.path.join(root, ".claude"))
    os.makedirs(os.path.join(root, "feedback"))
    os.symlink(os.path.join(feedback_repo, "feedback", "knowledge"),
               os.path.join(root, "feedback", "knowledge"))
    for form, (zip_path, pdfs) in packages.items():
        d = os.path.join(root, "forms", "issued", form)
        os.makedirs(d)
        shutil.copy2(zip_path, os.path.join(d, f"{form}_merged.zip"))
        for pdf, lang in pdfs:
            name = f"{form}_{lang}.pdf" if lang else f"{form}.pdf"
            shutil.copy2(pdf, os.path.join(d, name))
    return root


def run_guard(harness):
    """The guard's JSON report plus its exit code."""
    guard = os.path.join(harness, ".claude", "scripts", "check_regressions.py")
    if not os.path.isfile(guard):
        die(f"the feedback repo has no {os.path.relpath(guard, harness)}")
    r = subprocess.run(["python3", guard, "--no-skip", "--json"],
                       cwd=harness, capture_output=True, text=True)
    try:
        return json.loads(r.stdout or "{}"), r.returncode, r.stderr
    except json.JSONDecodeError:
        sys.stderr.write(r.stdout[-4000:])
        sys.stderr.write(r.stderr[-4000:])
        die("the guard's output was not JSON")


def count_enrolled(feedback_repo):
    """How many rules the guard enrols, counted the way it counts them itself: an entry
    whose **Status** line says "swept". The guard's JSON reports only the rules that were
    violated, so this is the denominator it does not hand back."""
    reg = os.path.join(feedback_repo, "feedback", "knowledge", "consistent-problems.md")
    try:
        text = open(reg, encoding="utf-8").read()
    except OSError:
        return 0
    n = 0
    for m in re.finditer(r"^## (PROBLEM-[a-z0-9][a-z0-9-]*) .*?(?=^## |\Z)", text, re.S | re.M):
        status = re.search(r"^\*\*Status:\*\*\s*(.*)$", m.group(0), re.M)
        if status and "swept" in status.group(1).lower():
            n += 1
    return n


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[1],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pdfs", nargs="+", help="source PDFs; grouped into forms by file name")
    ap.add_argument("--feedback-repo", default=DEFAULT_FEEDBACK_REPO,
                    help="checkout of ajila-forms-conversion-feedback (default: %(default)s)")
    ap.add_argument("--profile", default="ubs")
    ap.add_argument("--cli", default=None,
                    help="how to run the CLI (default: the release binary if built, else "
                         "`cargo run --release -p blueprint-cli --`)")
    ap.add_argument("--json", action="store_true", help="print the guard's report as JSON")
    ap.add_argument("--keep", action="store_true", help="keep the scratch directory")
    args = ap.parse_args()

    if not os.path.isdir(os.path.join(args.feedback_repo, ".claude", "scripts")):
        die(f"{args.feedback_repo} does not look like the feedback repo "
            f"(no .claude/scripts); pass --feedback-repo")

    if args.cli:
        cli = args.cli.split()
    else:
        built = os.path.join(ENGINE_ROOT, "target", "release", "blueprint-cli")
        cli = [built] if os.path.isfile(built) else \
            ["cargo", "run", "--quiet", "--release", "--manifest-path",
             os.path.join(ENGINE_ROOT, "Cargo.toml"), "-p", "blueprint-cli", "--"]

    scratch = tempfile.mkdtemp(prefix="feedback_check_")
    try:
        forms = group_by_form(args.pdfs)
        packages = {}
        for form, pdfs in forms.items():
            out = os.path.join(scratch, "convert", form)
            os.makedirs(out)
            print(f"converting {form} ({', '.join(l or '-' for _, l in pdfs)}) ...",
                  file=sys.stderr)
            packages[form] = (convert(pdfs, out, args.profile, cli), pdfs)

        harness = build_harness(os.path.join(scratch, "harness"), packages, args.feedback_repo)
        report, code, guard_stderr = run_guard(harness)

        if args.json:
            print(json.dumps(report, indent=2, sort_keys=True))
        else:
            sys.stderr.write(guard_stderr)
            violations = {k: v for k, v in (report.get("violations") or {}).items() if v}
            enrolled = count_enrolled(args.feedback_repo)
            print()
            if violations:
                print(f"{len(violations)} of {enrolled} enrolled rules violated:")
                for rule, forms_hit in sorted(violations.items()):
                    print(f"  {rule}: {', '.join(sorted(forms_hit))}")
            else:
                print(f"no violations: {enrolled} enrolled rules clean "
                      f"on {', '.join(sorted(packages))}")
        return code
    finally:
        if args.keep:
            print(f"scratch kept at {scratch}", file=sys.stderr)
        else:
            shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
