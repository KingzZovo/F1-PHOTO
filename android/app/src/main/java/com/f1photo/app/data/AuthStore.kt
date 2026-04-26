package com.f1photo.app.data

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.onStart

/**
 * Stores the JWT access token in EncryptedSharedPreferences and exposes a Flow
 * so navigation can react to login/logout. Backend login response field is
 * `access_token` (see server/src/api/auth.rs).
 */
class AuthStore(context: Context) {
    private val prefs: SharedPreferences = run {
        val masterKey = MasterKey.Builder(context)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build()
        EncryptedSharedPreferences.create(
            context,
            FILE_NAME,
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
    }

    val token: String? get() = prefs.getString(KEY_TOKEN, null)
    val username: String? get() = prefs.getString(KEY_USERNAME, null)

    val tokenFlow: Flow<String?> = callbackFlow {
        val listener = SharedPreferences.OnSharedPreferenceChangeListener { _, key ->
            if (key == KEY_TOKEN) trySend(prefs.getString(KEY_TOKEN, null))
        }
        prefs.registerOnSharedPreferenceChangeListener(listener)
        awaitClose { prefs.unregisterOnSharedPreferenceChangeListener(listener) }
    }.onStart { emit(prefs.getString(KEY_TOKEN, null)) }.distinctUntilChanged()

    fun saveToken(token: String, username: String) {
        prefs.edit().putString(KEY_TOKEN, token).putString(KEY_USERNAME, username).apply()
    }

    fun clear() {
        prefs.edit().clear().apply()
    }

    companion object {
        private const val FILE_NAME = "f1photo_secure_prefs"
        private const val KEY_TOKEN = "access_token"
        private const val KEY_USERNAME = "username"
    }
}
