package com.efi.androidflow.data

import android.content.Context
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.dataStore by preferencesDataStore(name = "flow_settings")

data class FlowSettings(
    val apiUrl: String = "",
    val apiKey: String = "",
    val language: String = "en",
    val correctEnglish: Boolean = true,
)

class SettingsRepository(private val context: Context) {
    private val apiUrlKey = stringPreferencesKey("api_url")
    private val apiKeyKey = stringPreferencesKey("api_key")
    private val languageKey = stringPreferencesKey("language")
    private val correctEnglishKey = booleanPreferencesKey("correct_english")

    val settingsFlow: Flow<FlowSettings> = context.dataStore.data.map { prefs ->
        FlowSettings(
            apiUrl = prefs[apiUrlKey].orEmpty(),
            apiKey = prefs[apiKeyKey].orEmpty(),
            language = prefs[languageKey] ?: "en",
            correctEnglish = prefs[correctEnglishKey] ?: true,
        )
    }

    suspend fun current(): FlowSettings = settingsFlow.first()

    suspend fun update(
        apiUrl: String? = null,
        apiKey: String? = null,
        language: String? = null,
        correctEnglish: Boolean? = null,
    ) {
        context.dataStore.edit { prefs ->
            apiUrl?.let { prefs[apiUrlKey] = it.trim().trimEnd('/') }
            apiKey?.let { prefs[apiKeyKey] = it.trim() }
            language?.let { prefs[languageKey] = it.trim() }
            correctEnglish?.let { prefs[correctEnglishKey] = it }
        }
    }
}
