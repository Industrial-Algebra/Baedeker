;; Threads delta from `binary.wast` — fork-only threads directives.
;;
;; Source: WebAssembly/threads@main-legacy 33b2dcd6 `test/core/binary.wast`
;; lines 586-595 (the single threads-related fork-only directive: a shared
;; table limits flag 0x02 is rejected with "tables cannot be shared").
;; Extracted verbatim; see docs/plans/threads-unit3-contract.md.

(assert_malformed
  (module binary
      "\00asm" "\01\00\00\00"
      "\04\04\01"                           ;; table section with one entry
      "\70"                                 ;; anyfunc
      "\02"                                 ;; malformed table limits flag
      "\00"                                 ;; min
  )
  "tables cannot be shared"
)
