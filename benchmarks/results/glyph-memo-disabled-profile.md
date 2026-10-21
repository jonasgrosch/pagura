# Disabled-path profile audit

**The profiles do not establish lazy memo initialization as the remedy.** They
show essentially the same broad render call tree. Lazy initialization remains
a source-motivated hypothesis, not a diagnosed cause or demonstrated speedup.

Reviewed retained artifacts under
`/Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura/benchmarks/results/local/disabled-regressions`:
control `baseline-profile-002` from `776fd4b9`, current `current-profile-001`
from `d16c07f8`. No executable, generator, build, test or profiler was run during
this audit. Only existing files were read/hashed, and this temporary report and
`glyph-memo-disabled-profile.json` were written.

## Integrity and execution records

All **45 recorded integrity/status checks matched**; details and complete hashes
are retained in `glyph-memo-disabled-profile.json`.

- Both record files bind the same plan SHA-256:
  `3171c77754fddb6e4c84ffb9beda69e1ec858f125e2bc54697119aca1178c9d1`.
- Actual profile binaries, driver, both copies of the Rust example, input PDF and
  retained independent PPM match their plan hashes. The two build receipts match
  their pinned hashes and recorded source identities.
- All four artifacts per profile match their record hashes. Each completed
  wrapper receipt's record/guard artifact hashes match the actual files.
- Both build/profile receipts report exit 0, unchanged source, no errors, and
  completed status. Both observation guards completed six scans and recorded no
  known competing job. This sampled observation does not establish exclusivity.
- Each profile contains all 3,000 indexed output rows and the expected FNV
  checksum in every row. The Rust loop additionally performs one unreported
  warmup render. FNV checksums are diagnostic consistency checks, not independent
  byte-equality proofs or a replacement for the prior accuracy gate.

The newly calculated receipt hashes are
`bd75cdd94db7818c8f29782010cdcd22429839b2dbf1df7b71c4f5f0e11aece9`
(control) and
`34e785e82ccf430ddf14e72eadd2a07684ccc5880fc25ccd07ef6132a548d063`
(current). They record what this reviewer read; they were not authenticated by
an external authority. Build receipts bind source but list no built binary
artifact, so source-to-binary correspondence retains the documented trusted
coordinator build/copy boundary.

The failed baseline-profile-001 attempt remains documented as a missing-plan
preflight failure before renderer launch; it is not a timing sample or a hidden
successful alternative.

## Comparable call-tree observations

Counts are sampled stacks, not call counts, render counts or milliseconds.
Inclusive and self counts below must not be added together. Current has 37
startup/loader samples in a separate root plus 3,830 main-loop samples; its
denominator is therefore 3,867, not just 3,830.

| Scope | Control samples | Current samples |
| --- | ---: | ---: |
| All captured main-thread stacks | 3,870 | 3,867 |
| `DisplayList::render`, inclusive | 3,032 (78.35%) | 3,026 (78.25%) |
| `Context::commands`, inclusive | 2,847 (73.57%) | 2,819 (72.90%) |
| Main regional `render_policy` branch, inclusive | 2,979 | 2,979 |
| `render_policy`, self | 132 | 160 |
| `Buffer::paint`, self | 736 | 688 |
| Image-source closure, self | 658 | 627 |
| `Context::image`, self | 629 | 638 |
| `Prepared::sample`, self | 439 | 495 |
| `Surface::index`, self | 159 | 175 |
| `transfer_mark`, self | 177 | 144 |
| Diagnostic `render_profile::main`, self | 834 | 793 |
| `_platform_memset`, self | 9 | 18 |

The image sampling/painting functions dominate both profiles. The main regional
branch contains exactly the same 2,979 samples, with 28 samples shifting from
commands into the enclosing `render_policy` self bucket. This does not localize
those samples to construction; inlining, code layout and sampling variation
prevent that inference.

There is no separately named `SceneIndex::new` or `Context::new` hot frame.
Visible memset paths predominantly belong to Pixmap filling and coverage work:
7 of the control's and 11 of current's memset samples sit under
`Pixmap::with_storage`/`filled`. None is explicitly attributed to memo storage.
Optimized/inlined zeroing could still be included in an enclosing frame, and
the absence of a named constructor cannot rule out a small cost.

## Limits and decision

These are two sequential five-second captures requested at one-millisecond
intervals, control first, with a long-lived process that opens and compiles once
then repeatedly renders the same image-only Exact page. They are not randomized
paired measurements and do not reproduce the earlier fresh-process workload.
Hashing and output occur outside each reported render timer but remain inside
the sampled process; about one fifth of samples are self time in the diagnostic
main function. Sample counts and reported process footprints cannot establish
latency or RSS improvements against the unprofiled confirmation. Tiny
Competitive cold/open/compile work is outside this profile's scope.

Source establishes unconditional initialization of 64 hull slots in
`SceneIndex::new`; it does not establish emitted zeroing cost after optimization.
A small safe implementation experiment could test lazy initialization while
preserving the fixed storage/work/error contract. These profiles alone do not
justify adding complexity, promising recovery of the image regression, or
claiming that stack footprint or process RSS would shrink. The prior retained
stage analysis also places most Tiny Competitive cold excess before rendering,
so constructor laziness cannot directly explain that whole observation.

Keep the prior protected regressions open. Any proposed change needs separately
frozen performance evidence with independent outputs and protected controls;
the diagnostic profiles neither reopen nor replace the completed confirmation.

An independent second static reader reached the same interpretation. No new
measurements or resampling were performed by either reviewer.
