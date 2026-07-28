;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/binary.wast

(module binary "\00asm\01\00\00\00")
(module binary "\00asm" "\01\00\00\00")

(assert_malformed (module binary "") "unexpected end")
(assert_malformed (module binary "\01") "unexpected end")
(assert_malformed (module binary "\00as") "unexpected end")
(assert_malformed (module binary "asm\00") "magic header not detected")
(assert_malformed (module binary "\00ASM\01\00\00\00") "magic header not detected")
