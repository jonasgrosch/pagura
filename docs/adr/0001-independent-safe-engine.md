# ADR 0001: Independent implementation with a safe Rust boundary

Status: accepted, 2026-10-17.

Pagura is written independently from published specifications and independently
constructed tests. Hayro is a competitor;
its implementation is not copied, translated, forked, wrapped, or reused.
ADR 0006 defines how other engines may inform the design: learn, never copy.
The same boundary applies to other PDF engines.

Every product Rust target forbids unsafe code. The bootstrap has no third-party
Cargo dependencies, proc macros, FFI or native build scripts. Rust std and the
compiler/platform are the trusted base; this is not a claim that the entire
operating system or standard library contains no unsafe implementation code.
Future dependencies must satisfy an explicit safety/provenance review.

External benchmark programs can have different languages and safety properties.
They run out of process and never become product dependencies. Their code and
licenses are stored separately from the product.

The cost is additional implementation effort and possible inability to meet a
performance target for some workloads. That outcome must remain visible; it is
not grounds to silently relax the project's safety or independence constraints.
