//! Standalone bindgen binary used to generate the Kotlin (and later Swift)
//! foreign-language bindings from the compiled `recall_core` cdylib. See
//! `core/uniffi.toml` for the Kotlin package name, and the crate's README /
//! Task 3 report for the exact invocation.
fn main() {
    uniffi::uniffi_bindgen_main()
}
