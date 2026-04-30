;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/imports.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/table.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/elem.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/type.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/type-equivalence.wast

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

(assert_invalid
  (module (type $type-func-param-invalid (func (param (ref 1)))))
  "unknown type"
)

(assert_invalid
  (module (type $type-func-result-invalid (func (result (ref 1)))))
  "unknown type"
)

(assert_invalid
  (module (global $global-invalid (ref null 1) (ref.null 1)))
  "unknown type"
)

(assert_invalid
  (module (table $table-invalid 10 (ref null 1)))
  "unknown type"
)

(assert_invalid
  (module (elem $elem-invalid (ref 1)))
  "unknown type"
)

(assert_invalid
  (module (func $func-param-invalid (param (ref 1))))
  "unknown type"
)

(assert_invalid
  (module (func $func-result-invalid (result (ref 1))))
  "unknown type"
)

(assert_invalid
  (module (func $func-local-invalid (local (ref null 1))))
  "unknown type"
)

(assert_invalid
  (module (func $block-result-invalid (drop (block (result (ref 1)) (unreachable)))))
  "unknown type"
)

(assert_invalid
  (module (func $loop-result-invalid (drop (loop (result (ref 1)) (unreachable)))))
  "unknown type"
)

(assert_invalid
  (module (func $if-invalid (drop (if (result (ref 1)) (then) (else)))))
  "unknown type"
)

(assert_invalid
  (module (func $select-result-invalid (drop (select (result (ref 1)) (unreachable)))))
  "unknown type"
)

(assert_invalid
  (module
    (type $t1 (func (param (ref $t2))))
    (type $t2 (func (param (ref $t1))))
  )
  "unknown type"
)
