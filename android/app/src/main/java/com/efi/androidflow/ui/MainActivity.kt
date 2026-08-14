package com.efi.androidflow.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.slideInVertically
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AccessibilityNew
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Cloud
import androidx.compose.material.icons.filled.Layers
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.RadioButtonUnchecked
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat
import com.efi.androidflow.FlowApp
import com.efi.androidflow.R
import com.efi.androidflow.data.FlowApiClient
import com.efi.androidflow.data.FlowLanguages
import com.efi.androidflow.data.FlowSettings
import com.efi.androidflow.service.BubbleOverlayService
import com.efi.androidflow.service.FlowAccessibilityService
import kotlinx.coroutines.launch

private val FlowTeal = Color(0xFF2DD4BF)
private val FlowTealDeep = Color(0xFF0F766E)
private val FlowInk = Color(0xFF071018)
private val FlowPanel = Color(0xCC0F1C2E)
private val FlowLine = Color(0x33FFFFFF)
private val FlowMist = Color(0xFF94A3B8)
private val FlowSnow = Color(0xFFF8FAFC)
private val FlowAmber = Color(0xFFFBBF24)

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        WindowCompat.getInsetsController(window, window.decorView).isAppearanceLightStatusBars = false
        setContent {
            AndroidFlowTheme {
                HubScreen()
            }
        }
    }
}

@Composable
fun AndroidFlowTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = androidx.compose.material3.darkColorScheme(
            primary = FlowTeal,
            onPrimary = FlowInk,
            secondary = FlowAmber,
            background = FlowInk,
            surface = FlowPanel,
            onSurface = FlowSnow,
            onBackground = FlowSnow,
        ),
        typography = MaterialTheme.typography.copy(
            displayLarge = MaterialTheme.typography.displayLarge.copy(
                fontFamily = FontFamily.Serif,
                fontWeight = FontWeight.Bold,
            ),
            headlineLarge = MaterialTheme.typography.headlineLarge.copy(
                fontFamily = FontFamily.Serif,
                fontWeight = FontWeight.SemiBold,
            ),
        ),
        content = content,
    )
}

@Composable
fun HubScreen() {
    val context = LocalContext.current
    val app = context.applicationContext as FlowApp
    val settings by app.settings.settingsFlow.collectAsState(initial = FlowSettings())
    val scope = rememberCoroutineScope()

    var apiUrl by remember(settings.apiUrl) { mutableStateOf(settings.apiUrl) }
    var apiKey by remember(settings.apiKey) { mutableStateOf(settings.apiKey) }
    var language by remember(settings.language) { mutableStateOf(settings.language) }
    var correctEnglish by remember(settings.correctEnglish) { mutableStateOf(settings.correctEnglish) }
    var healthOk by remember { mutableStateOf<Boolean?>(null) }
    var bubbleRunning by remember { mutableStateOf(false) }
    var visible by remember { mutableStateOf(false) }
    var showMicDisclosure by remember { mutableStateOf(false) }
    val privacyUrl = "https://shahaman098.github.io/android-flow/privacy.html"

    val micGranted = ContextCompat.checkSelfPermission(
        context,
        Manifest.permission.RECORD_AUDIO,
    ) == PackageManager.PERMISSION_GRANTED
    val overlayGranted = Settings.canDrawOverlays(context)
    val a11yGranted = FlowAccessibilityService.isEnabled()

    val micLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }

    val notificationLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }

    LaunchedEffect(Unit) {
        visible = true
        if (Build.VERSION.SDK_INT >= 33) {
            notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    val pulse = rememberInfiniteTransition(label = "pulse")
    val glow by pulse.animateFloat(
        initialValue = 0.35f,
        targetValue = 0.75f,
        animationSpec = infiniteRepeatable(
            animation = tween(2400, easing = FastOutSlowInEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "glow",
    )

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(
                Brush.verticalGradient(
                    listOf(Color(0xFF071018), Color(0xFF0B1F24), Color(0xFF071018)),
                ),
            ),
    ) {
        Box(
            modifier = Modifier
                .size(280.dp)
                .align(Alignment.TopEnd)
                .padding(top = 40.dp)
                .alpha(glow)
                .blur(90.dp)
                .background(FlowTealDeep.copy(alpha = 0.55f), CircleShape),
        )
        Box(
            modifier = Modifier
                .size(220.dp)
                .align(Alignment.BottomStart)
                .padding(bottom = 80.dp)
                .alpha(glow * 0.8f)
                .blur(80.dp)
                .background(Color(0xFF0369A1).copy(alpha = 0.4f), CircleShape),
        )

        AnimatedVisibility(
            visible = visible,
            enter = fadeIn(tween(500)) + slideInVertically(tween(600)) { it / 8 },
        ) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 22.dp, vertical = 48.dp),
                verticalArrangement = Arrangement.spacedBy(18.dp),
            ) {
                Text(
                    text = stringResource(R.string.app_name),
                    color = FlowSnow,
                    fontSize = 40.sp,
                    fontWeight = FontWeight.Bold,
                    fontFamily = FontFamily.Default,
                    letterSpacing = (-0.5).sp,
                )
                Text(
                    text = stringResource(R.string.hub_tagline),
                    color = FlowMist,
                    fontSize = 16.sp,
                    lineHeight = 22.sp,
                )

                SetupCard(
                    title = stringResource(R.string.hub_get_ready),
                    enableLabel = stringResource(R.string.hub_enable),
                    items = listOf(
                        SetupItem(stringResource(R.string.hub_permission_mic), micGranted, Icons.Filled.Mic) {
                            showMicDisclosure = true
                        },
                        SetupItem(stringResource(R.string.hub_permission_overlay), overlayGranted, Icons.Filled.Layers) {
                            val intent = Intent(
                                Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                                Uri.parse("package:${context.packageName}"),
                            )
                            context.startActivity(intent)
                        },
                        SetupItem(stringResource(R.string.hub_permission_a11y), a11yGranted, Icons.Filled.AccessibilityNew) {
                            context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
                        },
                        SetupItem(
                            stringResource(R.string.hub_permission_api),
                            settings.apiUrl.isNotBlank() && settings.apiKey.isNotBlank(),
                            Icons.Filled.Cloud,
                        ) { },
                    ),
                )

                Surface(
                    color = FlowPanel,
                    shape = RoundedCornerShape(24.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .border(1.dp, FlowLine, RoundedCornerShape(24.dp)),
                ) {
                    Column(
                        modifier = Modifier.padding(20.dp),
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Text(
                            text = stringResource(R.string.hub_connection),
                            color = FlowSnow,
                            fontSize = 20.sp,
                            fontWeight = FontWeight.SemiBold,
                            fontFamily = FontFamily.Default,
                        )
                        Text(
                            text = stringResource(R.string.hub_api_required),
                            color = FlowMist,
                            fontSize = 13.sp,
                            lineHeight = 18.sp,
                        )
                        FlowField(
                            value = apiUrl,
                            onValueChange = { apiUrl = it },
                            label = stringResource(R.string.hub_api_url),
                            placeholder = "https://flow-api-….run.app",
                        )
                        FlowField(
                            value = apiKey,
                            onValueChange = { apiKey = it },
                            label = stringResource(R.string.hub_api_key),
                            placeholder = "Bearer secret",
                        )
                        Text(
                            text = stringResource(R.string.hub_language),
                            color = FlowSnow,
                            fontWeight = FontWeight.Medium,
                        )
                        Text(
                            text = stringResource(R.string.hub_language_hint),
                            color = FlowMist,
                            fontSize = 13.sp,
                        )
                        LanguagePicker(
                            selected = FlowLanguages.normalize(language),
                            onSelect = { code ->
                                language = code
                                scope.launch {
                                    app.settings.update(language = code)
                                    FlowLanguages.applyAppLocale(code)
                                }
                            },
                        )
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Column(modifier = Modifier.weight(1f)) {
                                Text(stringResource(R.string.hub_cleanup), color = FlowSnow, fontWeight = FontWeight.Medium)
                                Text(stringResource(R.string.hub_cleanup_hint), color = FlowMist, fontSize = 13.sp)
                            }
                            Switch(
                                checked = correctEnglish,
                                onCheckedChange = { correctEnglish = it },
                                colors = SwitchDefaults.colors(
                                    checkedTrackColor = FlowTealDeep,
                                    checkedThumbColor = FlowTeal,
                                ),
                            )
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Button(
                                onClick = {
                                    scope.launch {
                                        val code = FlowLanguages.normalize(language)
                                        app.settings.update(apiUrl, apiKey, code, correctEnglish)
                                        FlowLanguages.applyAppLocale(code)
                                        Toast.makeText(
                                            context,
                                            context.getString(R.string.hub_saved),
                                            Toast.LENGTH_SHORT,
                                        ).show()
                                    }
                                },
                                colors = ButtonDefaults.buttonColors(
                                    containerColor = FlowTeal,
                                    contentColor = FlowInk,
                                ),
                                shape = RoundedCornerShape(14.dp),
                                modifier = Modifier.weight(1f),
                            ) { Text(stringResource(R.string.hub_save), fontWeight = FontWeight.Bold) }

                            TextButton(
                                onClick = {
                                    scope.launch {
                                        val code = FlowLanguages.normalize(language)
                                        app.settings.update(apiUrl, apiKey, code, correctEnglish)
                                        healthOk = runCatching {
                                            FlowApiClient(apiUrl.trim().trimEnd('/'), apiKey.trim()).health()
                                        }.getOrDefault(false)
                                    }
                                },
                            ) {
                                Text(
                                    when (healthOk) {
                                        true -> stringResource(R.string.hub_api_ok)
                                        false -> stringResource(R.string.hub_api_down)
                                        null -> stringResource(R.string.hub_test_api)
                                    },
                                    color = when (healthOk) {
                                        true -> FlowTeal
                                        false -> Color(0xFFF87171)
                                        null -> FlowMist
                                    },
                                )
                            }
                        }
                    }
                }

                Surface(
                    color = FlowPanel,
                    shape = RoundedCornerShape(24.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .border(1.dp, FlowLine, RoundedCornerShape(24.dp)),
                ) {
                    Column(
                        modifier = Modifier.padding(20.dp),
                        verticalArrangement = Arrangement.spacedBy(14.dp),
                    ) {
                        Text(
                            text = stringResource(R.string.hub_bubble),
                            color = FlowSnow,
                            fontSize = 20.sp,
                            fontWeight = FontWeight.SemiBold,
                            fontFamily = FontFamily.Default,
                        )
                        Text(
                            text = stringResource(R.string.hub_bubble_hint),
                            color = FlowMist,
                            fontSize = 14.sp,
                            lineHeight = 20.sp,
                        )
                        Button(
                            onClick = {
                                if (!micGranted) {
                                    showMicDisclosure = true
                                    return@Button
                                }
                                if (!overlayGranted) {
                                    context.startActivity(
                                        Intent(
                                            Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                                            Uri.parse("package:${context.packageName}"),
                                        ),
                                    )
                                    return@Button
                                }
                                if (bubbleRunning) {
                                    BubbleOverlayService.stop(context)
                                    bubbleRunning = false
                                } else {
                                    BubbleOverlayService.start(context)
                                    bubbleRunning = true
                                }
                            },
                            colors = ButtonDefaults.buttonColors(
                                containerColor = if (bubbleRunning) Color(0xFF7F1D1D) else FlowTeal,
                                contentColor = if (bubbleRunning) FlowSnow else FlowInk,
                            ),
                            shape = RoundedCornerShape(16.dp),
                            modifier = Modifier
                                .fillMaxWidth()
                                .height(54.dp),
                        ) {
                            Icon(
                                if (bubbleRunning) Icons.Rounded.Stop else Icons.Rounded.PlayArrow,
                                contentDescription = null,
                            )
                            Spacer(Modifier.width(8.dp))
                            Text(
                                if (bubbleRunning) {
                                    stringResource(R.string.hub_stop_bubble)
                                } else {
                                    stringResource(R.string.hub_launch_bubble)
                                },
                                fontWeight = FontWeight.Bold,
                                fontSize = 16.sp,
                            )
                        }
                    }
                }

                Text(
                    text = stringResource(R.string.hub_privacy_link),
                    color = FlowTeal,
                    fontSize = 14.sp,
                    modifier = Modifier.clickable {
                        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(privacyUrl)))
                    },
                )
                Text(
                    text = stringResource(R.string.hub_play_note),
                    color = FlowMist.copy(alpha = 0.8f),
                    fontSize = 12.sp,
                    textAlign = TextAlign.Start,
                    modifier = Modifier.padding(bottom = 24.dp),
                )
            }
        }

        if (showMicDisclosure) {
            AlertDialog(
                onDismissRequest = { showMicDisclosure = false },
                title = { Text(stringResource(R.string.mic_disclosure_title)) },
                text = { Text(stringResource(R.string.mic_disclosure_body)) },
                confirmButton = {
                    TextButton(
                        onClick = {
                            showMicDisclosure = false
                            micLauncher.launch(Manifest.permission.RECORD_AUDIO)
                        },
                    ) { Text(stringResource(R.string.mic_disclosure_continue), color = FlowTeal) }
                },
                dismissButton = {
                    TextButton(onClick = { showMicDisclosure = false }) {
                        Text(stringResource(R.string.mic_disclosure_cancel), color = FlowMist)
                    }
                },
                containerColor = FlowPanel,
                titleContentColor = FlowSnow,
                textContentColor = FlowMist,
            )
        }
    }
}

@Composable
private fun LanguagePicker(selected: String, onSelect: (String) -> Unit) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
        listOf(
            "en" to stringResource(R.string.hub_lang_en),
            "hi" to stringResource(R.string.hub_lang_hi),
            "ne" to stringResource(R.string.hub_lang_ne),
        ).forEach { (code, label) ->
            val active = selected == code
            Text(
                text = label,
                color = if (active) FlowInk else FlowSnow,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .weight(1f)
                    .clip(RoundedCornerShape(12.dp))
                    .background(if (active) FlowTeal else Color(0x22FFFFFF))
                    .border(1.dp, if (active) FlowTeal else FlowLine, RoundedCornerShape(12.dp))
                    .clickable { onSelect(code) }
                    .padding(vertical = 12.dp),
                textAlign = TextAlign.Center,
            )
        }
    }
}

data class SetupItem(
    val title: String,
    val done: Boolean,
    val icon: ImageVector,
    val onClick: () -> Unit,
)

@Composable
private fun SetupCard(title: String, enableLabel: String, items: List<SetupItem>) {
    Surface(
        color = FlowPanel,
        shape = RoundedCornerShape(24.dp),
        modifier = Modifier
            .fillMaxWidth()
            .border(1.dp, FlowLine, RoundedCornerShape(24.dp)),
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                text = title,
                color = FlowSnow,
                fontSize = 20.sp,
                fontWeight = FontWeight.SemiBold,
                fontFamily = FontFamily.Default,
            )
            items.forEach { item ->
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(14.dp))
                        .background(Color(0x14FFFFFF))
                        .padding(horizontal = 12.dp, vertical = 12.dp),
                ) {
                    Icon(item.icon, contentDescription = null, tint = FlowTeal, modifier = Modifier.size(22.dp))
                    Spacer(Modifier.width(12.dp))
                    Text(item.title, color = FlowSnow, modifier = Modifier.weight(1f))
                    if (item.done) {
                        Icon(Icons.Filled.CheckCircle, null, tint = FlowTeal)
                    } else {
                        TextButton(onClick = item.onClick) {
                            Text(enableLabel, color = FlowAmber)
                        }
                        Icon(Icons.Filled.RadioButtonUnchecked, null, tint = FlowMist)
                    }
                }
            }
        }
    }
}

@Composable
private fun FlowField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    placeholder: String,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        modifier = Modifier.fillMaxWidth(),
        label = { Text(label) },
        placeholder = { Text(placeholder) },
        singleLine = true,
        shape = RoundedCornerShape(14.dp),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = FlowTeal,
            unfocusedBorderColor = FlowLine,
            focusedLabelColor = FlowTeal,
            unfocusedLabelColor = FlowMist,
            cursorColor = FlowTeal,
            focusedTextColor = FlowSnow,
            unfocusedTextColor = FlowSnow,
            focusedContainerColor = Color(0x22000000),
            unfocusedContainerColor = Color(0x22000000),
        ),
    )
}
