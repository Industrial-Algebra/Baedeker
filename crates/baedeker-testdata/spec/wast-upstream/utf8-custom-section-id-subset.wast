;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/utf8-custom-section-id.wast

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\00\02"
    "\01\80"
  )
  "malformed UTF-8 encoding"
)
