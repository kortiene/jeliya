package dev.dioxus.main

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.annotation.Keep
import java.io.File

// Dioxus/wry 0.53.5's generated Logger.kt resolves this package-local alias.
// The default Dioxus MainActivity template supplies it; a custom activity must
// preserve it or Kotlin compilation fails even though the Rust library linked.
typealias BuildConfig = dev.jeliya.spike160.BuildConfig

/** Native test-only services for issue #160. No Flutter or Dart is involved. */
@Keep
class MainActivity : WryActivity() {
    private var pickerOpen = false
    private var resumedOnce = false

    override val handleBackNavigation: Boolean = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // One back authority. AppCompat's OnBackPressedDispatcher owns the
        // platform OnBackInvokedCallback registration, including predictive
        // Back on API 33+. Registering a second default-priority platform
        // callback races AppCompat: the first hardware run selected AppCompat's
        // fallback, moved the task itself, and never called nativeBackInvoked.
        // This callback is therefore the sole committed-Back handler.
        onBackPressedDispatcher.addCallback(
            this,
            object : OnBackPressedCallback(true) {
                override fun handleOnBackPressed() {
                    nativeBackInvoked()
                    finish()
                }
            },
        )
    }

    override fun onWebViewCreate(webView: WebView) {
        // The package version is evidence only; no floor is asserted by #160.
        nativePlatformReady(prepareProtectedState(), version)
    }

    override fun onResume() {
        super.onResume()
        if (resumedOnce) {
            nativeResumed()
        }
        resumedOnce = true
    }

    /**
     * Called reflectively by Rust through JNI; @Keep is release-critical.
     *
     * @Synchronized because there are two in-process callers on different
     * threads: onWebViewCreate on the Android main thread, and Rust boot() on a
     * tokio worker through JNI. Without serialization both can observe
     * !exists(), both call mkdir(), and the loser throws "could not create
     * protected state directory" — on the main-thread path that is an uncaught
     * crash, not a graceful failure. The monitor makes create-then-verify
     * atomic so the second caller deterministically takes the existing branch
     * and validates the marker. Fail-closed semantics are unchanged.
     */
    @Keep
    @Synchronized
    fun prepareProtectedState(): String {
        val root = noBackupFilesDir.canonicalFile
        val state = File(root, "dioxus-m0-spike-v1").canonicalFile
        if (state.parentFile != root) {
            throw IllegalStateException("protected state escaped noBackupFilesDir")
        }
        if (state.exists() && !state.isDirectory) {
            throw IllegalStateException("protected state path exists but is not a directory")
        }
        val created = if (!state.exists()) {
            if (!state.mkdir()) {
                throw IllegalStateException("could not create protected state directory")
            }
            true
        } else {
            false
        }

        val marker = File(state, "spike-test-data.json")
        val canonicalMarker = marker.canonicalFile
        if (canonicalMarker.parentFile != state) {
            throw IllegalStateException("protected marker escaped the state directory")
        }
        if (created) {
            if (marker.exists()) {
                throw IllegalStateException("new protected state unexpectedly contains a marker")
            }
            marker.writeText(
                """{"generation":"dioxus-m0-spike-v1","test_data":true}""",
                Charsets.UTF_8,
            )
        } else if (!marker.isFile) {
            // Never adopt or silently reinterpret an unverified directory.
            throw IllegalStateException("existing protected state has no regular generation marker")
        }
        return state.path
    }

    /**
     * Called reflectively by Rust through JNI; @Keep is release-critical.
     *
     * The JNI call originates on whatever thread Dioxus dispatches the click
     * on, which is not guaranteed to be the Android main thread. Launching an
     * activity-result flow and mutating pickerOpen off the main thread can
     * throw CalledFromWrongThread, so marshal onto the UI thread.
     * runOnUiThread executes inline when already on it, so the common case is
     * unaffected while the off-thread case becomes correct.
     */
    @Keep
    fun launchSafPicker() {
        runOnUiThread {
            if (pickerOpen) return@runOnUiThread
            pickerOpen = true
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "*/*"
            }
            startActivityForResult(intent, PICK_FILE_REQUEST)
        }
    }

    @Deprecated("Used for the disposable spike's single SAF request")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != PICK_FILE_REQUEST) return
        pickerOpen = false

        val uri: Uri? = data?.data
        if (resultCode != RESULT_OK || uri == null) {
            nativeSafResult("cancelled", "", "")
            return
        }

        var displayName = ""
        try {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) displayName = cursor.getString(0) ?: ""
                }
            // Prove the URI is readable without ever turning it into a fake path.
            contentResolver.openInputStream(uri)?.use { it.read(ByteArray(1)) }
                ?: throw IllegalStateException("content resolver returned no stream")
            nativeSafResult("selected", uri.toString(), displayName)
        } catch (error: Exception) {
            nativeSafResult("error", uri.toString(), error.javaClass.simpleName)
        }
    }

    companion object {
        private const val PICK_FILE_REQUEST = 160
        init {
            System.loadLibrary("jeliya_spike_160")
        }
    }

    private external fun nativePlatformReady(statePath: String, webViewVersion: String)
    private external fun nativeSafResult(status: String, uri: String, displayName: String)
    private external fun nativeResumed()
    private external fun nativeBackInvoked()
}
