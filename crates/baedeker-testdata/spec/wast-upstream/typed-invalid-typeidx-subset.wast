;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/imports.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/table.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/elem.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/type.wast

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\06\01\60\01\63\01\00\03\02\01\00\0a\07\01\05\00\20\00\1a\0b\00\0c\04\6e\61\6d\65\04\05\01\00\02\74\30"
  )
  "unknown type"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\02\0b\01\03\65\6e\76\01\67\03\63\01\00\00\0c\04\6e\61\6d\65\04\05\01\00\02\74\30"
  )
  "unknown type"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\04\05\01\63\01\00\01\00\0c\04\6e\61\6d\65\04\05\01\00\02\74\30"
  )
  "unknown type"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\09\08\01\05\63\01\01\d0\01\0b\00\0c\04\6e\61\6d\65\04\05\01\00\02\74\30"
  )
  "unknown type"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\07\01\05\01\01\63\01\0b\00\0c\04\6e\61\6d\65\04\05\01\00\02\74\30"
  )
  "unknown type"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\07\01\05\00\d0\01\1a\0b\00\0c\04\6e\61\6d\65\04\05\01\00\02\74\30"
  )
  "unknown type"
)

(assert_invalid
  (module binary
    "\00\61\73\6d\01\00\00\00\01\04\01\60\00\00\03\02\01\00\0a\0b\01\09\00\02\63\01\d0\01\1a\0b\0b\00\0c\04\6e\61\6d\65\04\05\01\00\02\74\30"
  )
  "unknown type"
)
