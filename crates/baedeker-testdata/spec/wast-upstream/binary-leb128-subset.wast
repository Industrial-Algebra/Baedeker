;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/binary-leb128.wast

(module binary
  "\00asm" "\01\00\00\00"
  "\05\04\01"
  "\00\82\00"
)

(module binary
  "\00asm" "\01\00\00\00"
  "\05\03\01"
  "\00\00"
  "\0b\07\01"
  "\80\00"
  "\41\00\0b\00"
)

(module binary
  "\00asm" "\01\00\00\00"
  "\01\08\01"
  "\60"
  "\82\00"
  "\7f\7e"
  "\01"
  "\7f"
)

(module binary
  "\00asm" "\01\00\00\00"
  "\01\05\01"
  "\60\01\7f\00"
  "\02\17\01"
  "\08"
  "\73\70\65\63\74\65\73\74"
  "\09"
  "\70\72\69\6e\74\5f\69\33\32"
  "\00"
  "\80\00"
)

(module binary
  "\00asm" "\01\00\00\00"
  "\01\04\01"
  "\60\00\00"
  "\03\03\01"
  "\80\00"
  "\0a\04\01"
  "\02\00\0b"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\05\08\01"
    "\00\82\80\80\80\80\80\80\80\80\80\00"
  )
  "integer representation too long"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\05\03\01"
    "\00\00"
    "\0b\0b\01"
    "\80\80\80\80\80\00"
    "\41\00\0b\00"
  )
  "integer representation too long"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\01\0c\01"
    "\60"
    "\82\80\80\80\80\00"
    "\7f\7e"
    "\01"
    "\7f"
  )
  "integer representation too long"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\01\05\01"
    "\60\01\7f\00"
    "\02\1b\01"
    "\88\80\80\80\80\00"
    "\73\70\65\63\74\65\73\74"
    "\09"
    "\70\72\69\6e\74\5f\69\33\32"
    "\00"
    "\00"
  )
  "integer representation too long"
)

(assert_malformed
  (module binary
    "\00asm" "\01\00\00\00"
    "\05\07\01"
    "\00\82\80\80\80\80\80\80\80\80\70"
  )
  "integer too large"
)
