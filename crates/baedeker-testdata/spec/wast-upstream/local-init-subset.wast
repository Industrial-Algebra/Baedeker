;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/local_init.wast

(module
  (func (param $p (ref extern)) (result (ref extern))
    (local $x (ref extern))
    (local.set $x (local.get $p))
    (local.get $x)
  )
  (func (param $p (ref extern)) (result (ref extern))
    (local $x (ref extern))
    (drop (local.tee $x (local.get $p)))
    (local.get $x)
  )
  (func (param $p (ref extern)) (result (ref extern))
    (local $x (ref extern))
    (local.set $x (local.get $p))
    (block (result (ref extern)) (local.get $x))
  ))

(assert_invalid
  (module (func (local $x (ref extern)) (drop (local.get $x))))
  "uninitialized local"
)

(assert_invalid
  (module
    (func (param $p (ref extern))
      (local $x (ref extern))
      (block (local.set $x (local.get $p)) (drop (local.tee $x (local.get $p))))
      (drop (local.get $x))
    )
  )
  "uninitialized local"
)

(assert_invalid
  (module
    (func (param $p (ref extern))
      (local $x (ref extern))
      (if (i32.const 0)
        (then (local.set $x (local.get $p)))
        (else (local.get $x))
      )
    )
  )
  "uninitialized local"
)

(assert_invalid
  (module
    (func (param $p (ref extern))
      (local $x (ref extern))
      (if (i32.const 0)
        (then (local.set $x (local.get $p)))
        (else (local.set $x (local.get $p)))
      )
      (drop (local.get $x))
    )
  )
  "uninitialized local"
)

(module
  (func (export "tee-init") (param $p (ref extern)) (result (ref extern))
    (local $x (ref extern))
    (drop (local.tee $x (local.get $p)))
    (local.get $x)
  ))
