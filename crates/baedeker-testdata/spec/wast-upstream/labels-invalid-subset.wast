;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/labels.wast

(assert_invalid
  (module
    (func
      (block $l
        f32.const 0
        br_if $l)))
  "type mismatch"
)

(assert_invalid
  (module
    (func
      (block $l (result i32)
        i32.const 0
        f32.const 0
        br_if $l)))
  "type mismatch"
)
