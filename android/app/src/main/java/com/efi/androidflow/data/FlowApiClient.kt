package com.efi.androidflow.data

import android.content.Context
import android.util.Base64
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.TimeUnit

data class ProcessResult(
    val text: String,
    val rawTranscript: String? = null,
)

class FlowApiClient(
    private val apiUrl: String,
    private val apiKey: String,
) {
    private val client = OkHttpClient.Builder()
        .connectTimeout(30, TimeUnit.SECONDS)
        .readTimeout(120, TimeUnit.SECONDS)
        .writeTimeout(120, TimeUnit.SECONDS)
        .build()

    private val jsonMedia = "application/json; charset=utf-8".toMediaType()

    suspend fun health(): Boolean = withContext(Dispatchers.IO) {
        val request = Request.Builder()
            .url("$apiUrl/livez")
            .get()
            .build()
        client.newCall(request).execute().use { it.isSuccessful }
    }

    suspend fun dictate(
        audioWav: ByteArray,
        language: String,
        dictionary: List<String> = emptyList(),
    ): ProcessResult = process(
        mode = "dictate",
        audioWav = audioWav,
        language = language,
        dictionary = dictionary,
    )

    suspend fun vibeText(
        selectedText: String,
        projectContext: String,
        constitution: String,
        skill: String,
        language: String,
        dictionary: List<String> = emptyList(),
    ): ProcessResult = process(
        mode = "vibe_text",
        selectedText = selectedText,
        projectContext = projectContext,
        constitution = constitution,
        skill = skill,
        language = language,
        dictionary = dictionary,
    )

    suspend fun correctText(
        selectedText: String,
        language: String,
        dictionary: List<String> = emptyList(),
    ): ProcessResult = process(
        mode = "correct_text",
        selectedText = selectedText,
        language = language,
        dictionary = dictionary,
    )

    private suspend fun process(
        mode: String,
        audioWav: ByteArray? = null,
        selectedText: String? = null,
        projectContext: String? = null,
        constitution: String? = null,
        skill: String? = null,
        language: String,
        dictionary: List<String>,
    ): ProcessResult = withContext(Dispatchers.IO) {
        require(apiUrl.isNotBlank()) { "Set FLOW_API_URL in Hub settings" }
        require(apiKey.isNotBlank()) { "Set FLOW_API_KEY in Hub settings" }

        val body = JSONObject().apply {
            put("mode", mode)
            put("language", language)
            put("dictionary", JSONArray(dictionary))
            if (audioWav != null) {
                put("audio_base64", Base64.encodeToString(audioWav, Base64.NO_WRAP))
            }
            if (!selectedText.isNullOrBlank()) put("selected_text", selectedText)
            if (!projectContext.isNullOrBlank()) put("project_context", projectContext)
            if (!constitution.isNullOrBlank()) put("constitution", constitution)
            if (!skill.isNullOrBlank()) put("skill", skill)
        }

        val request = Request.Builder()
            .url("$apiUrl/v1/process")
            .addHeader("Authorization", "Bearer $apiKey")
            .addHeader("Content-Type", "application/json")
            .post(body.toString().toRequestBody(jsonMedia))
            .build()

        client.newCall(request).execute().use { response ->
            val raw = response.body?.string().orEmpty()
            if (!response.isSuccessful) {
                val detail = runCatching {
                    JSONObject(raw).optString("detail", raw)
                }.getOrDefault(raw)
                throw IllegalStateException("flow-api ${response.code}: $detail")
            }
            val json = JSONObject(raw)
            ProcessResult(
                text = json.getString("text"),
                rawTranscript = json.optString("raw_transcript").ifBlank { null },
            )
        }
    }
}

object PromptAssets {
    fun loadProjectContext(context: Context): String =
        readAssetDir(context, "context")

    fun loadConstitution(context: Context): String =
        readAsset(context, "constitutions/vibe-coding.md")

    fun loadVibeSkill(context: Context): String =
        readAsset(context, "skills/vibe-prompt/SKILL.md")

    fun loadGrammarSkill(context: Context): String =
        readAsset(context, "skills/grammar-correct/SKILL.md")

    private fun readAssetDir(context: Context, dir: String): String {
        val names = context.assets.list(dir)?.sorted().orEmpty()
        return names.joinToString("\n\n") { name ->
            "# $name\n\n" + readAsset(context, "$dir/$name")
        }
    }

    private fun readAsset(context: Context, path: String): String =
        context.assets.open(path).bufferedReader().use { it.readText() }
}
