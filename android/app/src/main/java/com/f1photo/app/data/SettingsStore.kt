package com.f1photo.app.data

import android.content.Context
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import com.f1photo.app.BuildConfig
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

private val Context.dataStore by preferencesDataStore(name = "f1photo_settings")

/**
 * Persists user-overridable settings (currently just the server base URL and
 * default project ID). Built-in default base URL comes from BuildConfig.
 */
class SettingsStore(private val context: Context) {
    private object Keys {
        val BASE_URL: Preferences.Key<String> = stringPreferencesKey("base_url")
        val PROJECT_ID: Preferences.Key<String> = stringPreferencesKey("project_id")
    }

    val baseUrlFlow: Flow<String> = context.dataStore.data.map { prefs ->
        prefs[Keys.BASE_URL]?.takeIf { it.isNotBlank() } ?: BuildConfig.DEFAULT_BASE_URL
    }

    val projectIdFlow: Flow<String> = context.dataStore.data.map { prefs ->
        prefs[Keys.PROJECT_ID]?.takeIf { it.isNotBlank() } ?: DEFAULT_PROJECT_ID
    }

    suspend fun setBaseUrl(value: String) {
        context.dataStore.edit { it[Keys.BASE_URL] = value.trim() }
    }

    suspend fun setProjectId(value: String) {
        context.dataStore.edit { it[Keys.PROJECT_ID] = value.trim() }
    }

    companion object {
        // Mirrors the dev default project (see server seed data).
        const val DEFAULT_PROJECT_ID = "00000000-0000-0000-0000-000000000001"
    }
}
