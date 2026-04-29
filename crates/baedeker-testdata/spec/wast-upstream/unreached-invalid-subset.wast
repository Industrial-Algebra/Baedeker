;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/unreached-invalid.wast

(assert_invalid
  (module (func $local-index (unreachable) (drop (local.get 0))))
  "unknown local"
)

(assert_invalid
  (module (func $global-index (unreachable) (drop (global.get 0))))
  "unknown global"
)

(assert_invalid
  (module (func $func-index (unreachable) (call 1)))
  "unknown function"
)

(assert_invalid
  (module (func $label-index (unreachable) (br 1)))
  "unknown label"
)

(assert_invalid
  (module (func $type-poly-num-vs-num (result i32)
    (unreachable) (i64.const 0) (i32.const 0) (select)
  ))
  "type mismatch"
)

(assert_invalid
  (module (func $type-block-value-num-vs-num-after-break (result i32)
    (block (result i32) (i32.const 1) (br 0) (f32.const 0))
  ))
  "type mismatch"
)

(assert_invalid
  (module (func $type-binary-num-vs-num-after-return
    (return) (drop (f32.eq (i32.const 1) (f32.const 0)))
  ))
  "type mismatch"
)

(assert_invalid
  (module (func $type-if-value-num-vs-num-in-dead-body (result i32)
    (if (result i32) (i32.const 0) (then (f32.const 0)))
  ))
  "type mismatch"
)
