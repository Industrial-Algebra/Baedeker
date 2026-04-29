;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/unreached-valid.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/br_table.wast

(module
  (func (param $cond i32) (result i32)
    (select (unreachable) (i32.const 0) (local.get $cond)))
  (func (param $cond i32) (result i32)
    (select (i32.const 0) (unreachable) (local.get $cond)))

  (func
    (unreachable) (select)
    (unreachable) (i32.const 0) (select)
    (unreachable) (i32.const 0) (i32.const 0) (select)
    (unreachable) (i32.const 0) (i32.const 0) (i32.const 0) (select)
    (unreachable) (f32.const 0) (i32.const 0) (select)
    (unreachable)
  )

  (func (result i32)
    (unreachable) (i32.add (select))
  )

  (func (result i64)
    (unreachable) (i64.add (select (i64.const 0) (i32.const 0)))
  )

  (func
    (unreachable)
    (select)
    (i32.eqz)
    (drop)
  )
  (func
    (unreachable)
    (select)
    (ref.is_null)
    (drop)
  )

  (type $t (func (param i32) (result i32)))
  (func (result i32)
    (unreachable)
    (call_ref $t)
  )
)

(module
  (func (result (ref func))
    (unreachable)
    (ref.as_non_null)
  )
  (func (result (ref extern))
    (unreachable)
    (ref.as_non_null)
  )

  (func (result (ref func))
    (block (result funcref)
      (unreachable)
      (br_on_null 0)
      (return)
    )
    (unreachable)
  )
  (func (result (ref extern))
    (block (result externref)
      (unreachable)
      (br_on_null 0)
      (return)
    )
    (unreachable)
  )
)

(module
  (func
    (block (result f64)
      (block (result f32)
        (unreachable)
        (br_table 0 1 1 (i32.const 1))
      )
      (drop)
      (f64.const 0)
    )
    (drop)
  )
)
