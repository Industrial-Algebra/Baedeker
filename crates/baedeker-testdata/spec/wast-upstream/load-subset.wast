;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/load.wast

(module
  (memory 1)

  (func (export "as-br-value") (result i32)
    (block (result i32) (br 0 (i32.load (i32.const 0))))
  )

  (func (export "as-br_if-cond")
    (block (br_if 0 (i32.load (i32.const 0))))
  )

  (func (export "as-br_if-value") (result i32)
    (block (result i32)
      (drop (br_if 0 (i32.load (i32.const 0)) (i32.const 1))) (i32.const 7)
    )
  ))

(assert_invalid
  (module (memory 1) (func $load_i32 (i32.load (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module (memory 1) (func (result i32) (i32.load (f32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (memory 0)
    (func $type-address-empty-in-block
      (i32.const 0)
      (block (i32.load) (drop))))
  "type mismatch"
)
