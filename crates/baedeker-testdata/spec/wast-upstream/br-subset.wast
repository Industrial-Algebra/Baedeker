;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/br.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/loop.wast

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t1)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t0))
    (block (result (ref null $t0))
      (ref.null $t0)
      (loop (param (ref null $t0)) (result (ref null $t0))
        (drop)
        (ref.func $f)
        (br 0)))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f (type $t1)
      (local.get 0))
    (export "f" (func $f))
    (func (result (ref null $t0))
      (block (result (ref null $t0))
        (ref.null $t0)
        (loop (param (ref null $t0)) (result (ref null $t0))
          (drop)
          (ref.func $f)
          (br 0)))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f1 (type $t1)
    (local.get 0))
  (export "f1" (func $f1))
  (func (result (ref null $t0))
    (block (result (ref null $t0))
      (br 0 (ref.func $f1))
      (ref.null $t1))))

(assert_invalid
  (module
    (type $t0 (func (param i64) (result i64)))
    (type $t1 (func (param i32) (result i32)))
    (func $f1 (type $t1)
      (local.get 0))
    (export "f1" (func $f1))
    (func (result (ref null $t1))
      (block (result (ref null $t1))
        (br 0 (ref.func $f1))
        (ref.null $t0))))
  "type mismatch"
)

(assert_invalid
  (module (func $type-arg-empty-vs-num (result i32)
    (block (result i32) (br 0) (i32.const 1))
  ))
  "type mismatch"
)

(assert_invalid
  (module (func $type-arg-num-vs-num (result i32)
    (block (result i32) (br 0 (i64.const 1)) (i32.const 1))
  ))
  "type mismatch"
)
