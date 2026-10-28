;; Threads delta from `memory.wast` — fork-only threads directives.
;;
;; Source: WebAssembly/threads@main-legacy 33b2dcd6 `test/core/memory.wast`
;; lines 9, 10 (shared memory types) and 12 (shared memory without a
;; maximum is invalid: "shared memory must have maximum"). Extracted
;; verbatim; see docs/plans/threads-unit3-contract.md.

(module (memory 0 0 shared))
(module (memory 1 2 shared))
(assert_invalid (module (memory 1 shared)) "shared memory must have maximum")
