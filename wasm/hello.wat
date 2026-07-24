;; NWarp example WASM request handler.
;;
;; This module implements NWarp's WASM handler ABI (see
;; docs/ARCHITECTURE.md, Phase 6):
;;
;;   memory        - exported linear memory the host reads/writes into
;;   alloc(size)   - bump-allocates `size` bytes, returns a pointer
;;   handle(method_ptr, method_len, path_ptr, path_len) -> i64
;;                 - packs (response_ptr << 32 | response_len)
;;
;; Response bytes at response_ptr: first 2 bytes are the HTTP status
;; code (u16 little-endian), the rest is the response body.
;;
;; This handler echoes the requested path back in its response body,
;; to demonstrate that the host is genuinely passing real per-request
;; data into the sandboxed module rather than returning a static string.
(module
  (memory (export "memory") 2)
  (data (i32.const 0) "Hello from a sandboxed WASM module! You requested: ")

  ;; simple bump allocator - starts well past the data segment above
  (global $heap_ptr (mut i32) (i32.const 4096))

  (func $alloc (export "alloc") (param $size i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $heap_ptr))
    (global.set $heap_ptr (i32.add (global.get $heap_ptr) (local.get $size)))
    (local.get $ptr)
  )

  (func $handle (export "handle")
        (param $method_ptr i32) (param $method_len i32)
        (param $path_ptr i32) (param $path_len i32)
        (result i64)
    (local $prefix_len i32)
    (local $total_len i32)
    (local $buf i32)

    (local.set $prefix_len (i32.const 51))
    (local.set $total_len
      (i32.add (i32.const 2) (i32.add (local.get $prefix_len) (local.get $path_len))))
    (local.set $buf (call $alloc (local.get $total_len)))

    ;; status 200, little-endian u16, at buf+0
    (i32.store16 (local.get $buf) (i32.const 200))

    ;; copy the fixed prefix string into buf+2
    (memory.copy
      (i32.add (local.get $buf) (i32.const 2))
      (i32.const 0)
      (local.get $prefix_len))

    ;; copy the actual requested path (real per-request data, supplied
    ;; by the host from the incoming HTTP request) right after it
    (memory.copy
      (i32.add (local.get $buf) (i32.add (i32.const 2) (local.get $prefix_len)))
      (local.get $path_ptr)
      (local.get $path_len))

    ;; pack (buf << 32) | total_len into the i64 return value
    (i64.or
      (i64.shl (i64.extend_i32_u (local.get $buf)) (i64.const 32))
      (i64.extend_i32_u (local.get $total_len)))
  )
)

