;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/imports.wast

(assert_invalid
  (module binary
    "\00asm" "\01\00\00\00"
    "\01\05\01\60\00\01\7f"
    "\02\0d\01\04test\04func\00\01"
  )
  "unknown type"
)
