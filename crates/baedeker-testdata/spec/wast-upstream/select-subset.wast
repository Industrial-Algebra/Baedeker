;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/select.wast

(assert_invalid
  (module (func $arity-0-implicit (select (nop) (nop) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module (func $arity-0 (select (result) (nop) (nop) (i32.const 1))))
  "invalid result arity"
)

(assert_invalid
  (module
    (func $type-mismatch
      (drop (select (i32.const 1) (f32.const 2) (i32.const 0))))
  )
  "type mismatch"
)
