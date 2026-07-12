# Recall app ProGuard/R8 rules. Keep rules for JNA + UniFFI go here when minification is enabled.

-keep class com.sun.jna.** { *; }
-keep class com.recall.core.** { *; }
