package com.incubtek.jeliya_app

import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.android.FlutterActivity
import io.flutter.plugin.common.MethodChannel
import java.io.File

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, STORAGE_CHANNEL)
            .setMethodCallHandler { call, result ->
                if (call.method != "protectedEngineDataDir") {
                    result.notImplemented()
                    return@setMethodCallHandler
                }
                try {
                    result.success(prepareProtectedEngineDataDir())
                } catch (error: Exception) {
                    // Fail closed: starting with a fresh identity after a failed
                    // migration would silently sever the user's room identity.
                    result.error("protected_storage_unavailable", error.message, null)
                }
            }
    }

    private fun prepareProtectedEngineDataDir(): String {
        val protectedDir = File(noBackupFilesDir, "engine")
        val legacyDir = File(filesDir, "engine")

        if (legacyDir.exists()) {
            if (!legacyDir.isDirectory) {
                throw IllegalStateException("legacy engine path is not a directory")
            }
            if (protectedDir.exists()) {
                throw IllegalStateException(
                    "both legacy and protected engine directories exist; refusing ambiguous identity migration",
                )
            }
            if (!legacyDir.renameTo(protectedDir)) {
                throw IllegalStateException("could not move the legacy engine directory into protected storage")
            }
        } else if (!protectedDir.exists() && !protectedDir.mkdirs()) {
            throw IllegalStateException("could not create the protected engine directory")
        }
        if (!protectedDir.isDirectory) {
            throw IllegalStateException("protected engine path is not a directory")
        }

        val parent = protectedDir.canonicalFile.parentFile
        if (parent != noBackupFilesDir.canonicalFile) {
            throw IllegalStateException("protected engine directory escaped noBackupFilesDir")
        }
        return protectedDir.canonicalPath
    }

    companion object {
        private const val STORAGE_CHANNEL = "com.incubtek.jeliya/storage"
    }
}
