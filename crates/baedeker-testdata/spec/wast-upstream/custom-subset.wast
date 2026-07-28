;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/custom.wast

(module binary
  "\00asm" "\01\00\00\00"
  "\00\0e\06" "custom" "payload"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\00"
  )
  "unexpected end"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\00\26\10" "a custom section" "this is the payload"
  )
  "length out of bounds"
)
