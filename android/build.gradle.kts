// Nothing is applied at the root: every plugin is declared `apply false` so a
// module that does not want the Android plugin does not get it. `:core` is
// exactly such a module, and that is what lets its tests run with no SDK.
plugins {
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.kotlin.serialization) apply false
}
