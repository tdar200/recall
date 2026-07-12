package com.recall.app

import android.app.Application

/**
 * Application entry point. Holds the single [RecallRepository] instance for the
 * process lifetime; the repository itself constructs the native [com.recall.core.RecallEngine]
 * lazily (and guardedly) so app launch never crashes on a missing/unready engine.
 */
class RecallApp : Application() {
    val repository: RecallRepository by lazy { RecallRepository(this) }
}
