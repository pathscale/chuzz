;; A guest that builds one page through the blitz-wasm ABI, for testing
;; `capture::capture_wasm`.
;;
;; The page, which is what the tree dump and the PNG are asserted against:
;;
;;   <div class="panel" id="root">
;;     <h1>Blitz</h1>
;;     <p class="row">one</p>
;;     <p class="row">two</p>
;;     <p class="row">three</p>
;;   </div>
;;
;; WAT rather than a Rust guest compiled to wasm32, for three reasons.
;;
;; 1. It belongs to chuzz. `blitz-wasm` ships its own demo guest, and that guest
;;    is a fixture for *its* tests: it is free to become a counter, or a
;;    reactive benchmark, or anything else that proves the binding. When it
;;    does, a chuzz test that had borrowed it fails for a reason that has
;;    nothing to do with chuzz.
;; 2. It needs no toolchain. A Rust guest means `rustup target add
;;    wasm32-unknown-unknown` and a nested `cargo build` on a second workspace;
;;    this is assembled in-process by the `wat` crate.
;; 3. It is pinned to the ABI rather than to a binding crate. These six imports
;;    are what `blitz_wasm::add_to_linker` registers. If a signature there
;;    changes, this module stops instantiating, which is exactly the drift a
;;    capture test should notice.
;;
;; Handle 0 is the mount point the host seeds every instance with
;; (`blitz_wasm::MOUNT`), so it is the parent the finished panel is appended to.
(module
  (import "blitz" "intern" (func $intern (param i32 i32) (result i32)))
  (import "blitz" "create_element" (func $create_element (param i32) (result i32)))
  (import "blitz" "create_text" (func $create_text (param i32 i32) (result i32)))
  (import "blitz" "append_child" (func $append_child (param i32 i32) (result i32)))
  (import "blitz" "set_attribute" (func $set_attribute (param i32 i32 i32) (result i32)))

  ;; Exported as "memory" because that is the export `read_string` looks up by
  ;; name when it copies a string out of the guest.
  (memory (export "memory") 1)

  ;;                    0    3      8     13  15    19  21 22   25     30   33   36
  (data (i32.const 0) "divclasspanelidrooth1prowBlitzonetwothree")

  ;; The first negative status any host call returned, which is what `run`
  ;; reports. A status code, never a trap: the ABI forbids the guest from
  ;; taking the instance down, and a fixture that trapped would be testing the
  ;; host's panic path instead of its render path.
  (global $failed (mut i32) (i32.const 0))
  (global $status (mut i32) (i32.const 0))

  ;; Pass a host result through, recording it if it is the first failure.
  (func $note (param $value i32) (result i32)
    (if
      (i32.and
        (i32.lt_s (local.get $value) (i32.const 0))
        (i32.eqz (global.get $failed)))
      (then
        (global.set $failed (i32.const 1))
        (global.set $status (local.get $value))))
    (local.get $value))

  ;; <p class="row">TEXT</p>, appended to $parent.
  (func $row
    (param $parent i32) (param $p i32) (param $class i32) (param $row i32)
    (param $text i32) (param $text_len i32)
    (local $element i32)
    (local.set $element (call $note (call $create_element (local.get $p))))
    (drop
      (call $note
        (call $set_attribute (local.get $element) (local.get $class) (local.get $row))))
    (drop
      (call $note
        (call $append_child
          (local.get $element)
          (call $note (call $create_text (local.get $text) (local.get $text_len))))))
    (drop (call $note (call $append_child (local.get $parent) (local.get $element)))))

  (func (export "run") (result i32)
    (local $div i32) (local $class i32) (local $panel_class i32) (local $id i32)
    (local $root i32) (local $h1 i32) (local $p i32) (local $row i32)
    (local $panel i32) (local $heading i32)

    ;; Every name crosses the boundary exactly once, here. `class` and `row`
    ;; are interned before the rows rather than inside them, which is the whole
    ;; point of the atom tier: the second and third row cost nothing.
    (local.set $div (call $note (call $intern (i32.const 0) (i32.const 3))))
    (local.set $class (call $note (call $intern (i32.const 3) (i32.const 5))))
    (local.set $panel_class (call $note (call $intern (i32.const 8) (i32.const 5))))
    (local.set $id (call $note (call $intern (i32.const 13) (i32.const 2))))
    (local.set $root (call $note (call $intern (i32.const 15) (i32.const 4))))
    (local.set $h1 (call $note (call $intern (i32.const 19) (i32.const 2))))
    (local.set $p (call $note (call $intern (i32.const 21) (i32.const 1))))
    (local.set $row (call $note (call $intern (i32.const 22) (i32.const 3))))

    ;; <div class="panel" id="root">
    (local.set $panel (call $note (call $create_element (local.get $div))))
    (drop
      (call $note
        (call $set_attribute (local.get $panel) (local.get $class) (local.get $panel_class))))
    (drop
      (call $note
        (call $set_attribute (local.get $panel) (local.get $id) (local.get $root))))

    ;; <h1>Blitz</h1>
    (local.set $heading (call $note (call $create_element (local.get $h1))))
    (drop
      (call $note
        (call $append_child
          (local.get $heading)
          (call $note (call $create_text (i32.const 25) (i32.const 5))))))
    (drop (call $note (call $append_child (local.get $panel) (local.get $heading))))

    ;; Three rows, so the interesting cost is what the second and third add.
    (call $row (local.get $panel) (local.get $p) (local.get $class) (local.get $row)
      (i32.const 30) (i32.const 3))
    (call $row (local.get $panel) (local.get $p) (local.get $class) (local.get $row)
      (i32.const 33) (i32.const 3))
    (call $row (local.get $panel) (local.get $p) (local.get $class) (local.get $row)
      (i32.const 36) (i32.const 5))

    ;; Onto the mount point, which is where a detached tree becomes a page.
    (drop (call $note (call $append_child (i32.const 0) (local.get $panel))))
    (global.get $status))
)
