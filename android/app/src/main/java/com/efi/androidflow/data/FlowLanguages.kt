package com.efi.androidflow.data

import androidx.appcompat.app.AppCompatDelegate
import androidx.core.os.LocaleListCompat

object FlowLanguages {
    data class Option(val code: String, val labelEn: String)

    val options = listOf(
        Option("en", "English"),
        Option("hi", "हिन्दी"),
        Option("ne", "नेपाली"),
    )

    fun normalize(code: String): String {
        val raw = code.trim().lowercase().replace('_', '-')
        return when {
            raw.startsWith("hi") -> "hi"
            raw.startsWith("ne") -> "ne"
            raw.startsWith("en") || raw.isBlank() || raw == "auto" -> "en"
            else -> raw.take(2)
        }
    }

    fun applyAppLocale(code: String) {
        val tag = normalize(code)
        AppCompatDelegate.setApplicationLocales(LocaleListCompat.forLanguageTags(tag))
    }

    /** Clear instruction for LLMs — not just a bare BCP-47 tag. */
    fun llmInstruction(code: String): String =
        when (normalize(code)) {
            "hi" ->
                "Respond in fluent Hindi (हिन्दी). Prefer Devanagari script. " +
                    "Match the user's mix of Hindi/English (Hinglish) if they used both."
            "ne" ->
                "Respond in fluent Nepali (नेपाली). Prefer Devanagari script. " +
                    "Match the user's mix of Nepali/English if they used both."
            else ->
                "Respond in clear English matching the user's dialect."
        }
}
