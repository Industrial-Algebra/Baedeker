;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/block.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/call_ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/return_call_ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/br_on_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/br_on_non_null.wast

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\04\01\02\00\14"
  )
  "unexpected end"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\04\01\02\00\15"
  )
  "unexpected end"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\04\01\02\00\d5"
  )
  "unexpected end"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\04\01\02\00\d6"
  )
  "unexpected end"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\04\01\02\00\d0"
  )
  "unexpected end"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\05\01\03\00\02\63"
  )
  "unexpected end"
)
