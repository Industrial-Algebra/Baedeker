;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/utf8-import-module.wast

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\02\0b"
    "\01"
    "\04\74\65\73\74"
    "\01\80"
    "\03"
    "\7f"
    "\00"
  )
  "malformed UTF-8 encoding"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\02\0b"
    "\01"
    "\04\74\65\73\74"
    "\01\bf"
    "\03"
    "\7f"
    "\00"
  )
  "malformed UTF-8 encoding"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\02\0d"
    "\01"
    "\04\74\65\73\74"
    "\03\c2\80\80"
    "\03"
    "\7f"
    "\00"
  )
  "malformed UTF-8 encoding"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\02\0b"
    "\01"
    "\04\74\65\73\74"
    "\01\c2"
    "\03"
    "\7f"
    "\00"
  )
  "malformed UTF-8 encoding"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\02\0c"
    "\01"
    "\04\74\65\73\74"
    "\02\c0\80"
    "\03"
    "\7f"
    "\00"
  )
  "malformed UTF-8 encoding"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\02\0d"
    "\01"
    "\04\74\65\73\74"
    "\03\ed\a0\80"
    "\03"
    "\7f"
    "\00"
  )
  "malformed UTF-8 encoding"
)
