;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/block.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/if.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_func.wast

(module
  (func $f)
  (elem declare func $f)

  (func (result funcref)
    (block (result funcref)
      (br 0 (ref.func $f))
      (ref.null func)))

  (func (param i32) (result externref)
    (if (result externref)
      (local.get 0)
      (then (ref.null extern))
      (else (ref.null extern))))

  (func (result i32)
    (ref.is_null
      (block (result funcref)
        (br 0 (ref.null func))
        (ref.func $f)))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (result (ref null $t1))
    (block (result (ref null $t1))
      (br 0 (ref.func $f))
      (ref.null $t1))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param i32) (result (ref null $t1))
    (if (result (ref null $t1))
      (local.get 0)
      (then (ref.func $f))
      (else (ref.null $t1)))))

(assert_invalid
  (module
    (func $f)
    (elem declare func $f)
    (func (param i32) (result funcref)
      (block (result funcref)
        (br_if 0 (ref.null extern) (local.get 0))
        (ref.null func))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (param i32) (result funcref)
      (if (result funcref)
        (local.get 0)
        (then (ref.null func))
        (else (ref.null extern)))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i64)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param i32) (result (ref null $t1))
      (if (result (ref null $t1))
        (local.get 0)
        (then (ref.func $f))
        (else (ref.null $t1)))))
  "type mismatch"
)
