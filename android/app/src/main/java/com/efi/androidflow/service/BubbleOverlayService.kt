package com.efi.androidflow.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.PixelFormat
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import androidx.core.app.NotificationCompat
import com.efi.androidflow.FlowApp
import com.efi.androidflow.R
import com.efi.androidflow.audio.MicRecorder
import com.efi.androidflow.data.FlowApiClient
import com.efi.androidflow.data.PromptAssets
import com.efi.androidflow.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.abs

enum class BubbleState { Idle, Recording, Processing, Error }

class BubbleOverlayService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mainHandler = Handler(Looper.getMainLooper())
    private lateinit var windowManager: WindowManager
    private var overlayView: View? = null
    private var panelExpanded = false
    private var state = BubbleState.Idle
    private var statusLabel: TextView? = null
    private var bubbleButton: ImageButton? = null
    private var panel: LinearLayout? = null
    private val recorder = MicRecorder()
    private var pumpJob: Job? = null
    private var lastError: String? = null
    private var holdRunnable: Runnable? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        windowManager = getSystemService(WINDOW_SERVICE) as WindowManager
        startForeground(NOTIFICATION_ID, buildNotification())
        showOverlay()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) stopSelf()
        return START_STICKY
    }

    override fun onDestroy() {
        holdRunnable?.let { mainHandler.removeCallbacks(it) }
        recorder.cancel()
        pumpJob?.cancel()
        overlayView?.let { runCatching { windowManager.removeView(it) } }
        overlayView = null
        scope.cancel()
        super.onDestroy()
    }

    private fun showOverlay() {
        val root = FrameLayout(this)

        panel = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundResource(R.drawable.bg_bubble_panel)
            setPadding(dp(14), dp(12), dp(14), dp(12))
            visibility = View.GONE
            elevation = dp(10).toFloat()
        }

        statusLabel = TextView(this).apply {
            text = "Ready"
            setTextColor(0xFFE2E8F0.toInt())
            textSize = 13f
            setPadding(0, 0, 0, dp(8))
        }
        panel?.addView(statusLabel)
        panel?.addView(actionButton(getString(R.string.bubble_vibe)) { runVibe() })
        panel?.addView(actionButton(getString(R.string.bubble_grammar)) { runGrammar() })
        panel?.addView(
            actionButton(getString(R.string.bubble_open_hub)) {
                startActivity(
                    Intent(this@BubbleOverlayService, MainActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            },
        )

        bubbleButton = ImageButton(this).apply {
            setBackgroundResource(R.drawable.bg_bubble)
            setImageResource(R.drawable.ic_mic)
            scaleType = android.widget.ImageView.ScaleType.CENTER_INSIDE
            setPadding(dp(14), dp(14), dp(14), dp(14))
            contentDescription = getString(R.string.app_name)
        }

        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.END
            addView(panel)
            addView(
                bubbleButton,
                LinearLayout.LayoutParams(dp(58), dp(58)).apply {
                    topMargin = dp(8)
                    gravity = Gravity.END
                },
            )
        }
        root.addView(column)

        val params = WindowManager.LayoutParams(
            WindowManager.LayoutParams.WRAP_CONTENT,
            WindowManager.LayoutParams.WRAP_CONTENT,
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = dp(24)
            y = dp(180)
        }

        var originX = 0
        var originY = 0
        var startRawX = 0f
        var startRawY = 0f
        var moved = false
        var holdArmed = false

        bubbleButton?.setOnTouchListener { _, event ->
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    originX = params.x
                    originY = params.y
                    startRawX = event.rawX
                    startRawY = event.rawY
                    moved = false
                    holdArmed = state == BubbleState.Idle
                    holdRunnable?.let { mainHandler.removeCallbacks(it) }
                    if (holdArmed) {
                        val startHold = Runnable {
                            if (holdArmed && !moved && state == BubbleState.Idle) {
                                startDictation()
                            }
                        }
                        holdRunnable = startHold
                        mainHandler.postDelayed(startHold, 280L)
                    }
                    true
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = (event.rawX - startRawX).toInt()
                    val dy = (event.rawY - startRawY).toInt()
                    if (abs(dx) > dp(8) || abs(dy) > dp(8)) {
                        moved = true
                        holdArmed = false
                        holdRunnable?.let { mainHandler.removeCallbacks(it) }
                        if (state == BubbleState.Recording) {
                            // keep recording while dragging
                        }
                    }
                    params.x = originX + dx
                    params.y = originY + dy
                    windowManager.updateViewLayout(root, params)
                    true
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    holdRunnable?.let { mainHandler.removeCallbacks(it) }
                    holdRunnable = null
                    when {
                        state == BubbleState.Recording -> stopDictationAndProcess()
                        !moved && state == BubbleState.Idle -> togglePanel()
                    }
                    holdArmed = false
                    true
                }
                else -> false
            }
        }

        overlayView = root
        windowManager.addView(root, params)
        updateChrome()
    }

    private fun actionButton(label: String, onClick: () -> Unit): TextView =
        TextView(this).apply {
            text = label
            setTextColor(0xFFFFFFFF.toInt())
            textSize = 14f
            setPadding(dp(8), dp(10), dp(8), dp(10))
            setOnClickListener { onClick() }
        }

    private fun togglePanel() {
        panelExpanded = !panelExpanded
        panel?.visibility = if (panelExpanded) View.VISIBLE else View.GONE
    }

    private fun setState(next: BubbleState, message: String? = null) {
        state = next
        if (message != null) lastError = message
        updateChrome()
    }

    private fun updateChrome() {
        val (color, label) = when (state) {
            BubbleState.Idle -> 0xFF0D9488.toInt() to getString(R.string.bubble_ready)
            BubbleState.Recording -> 0xFFF59E0B.toInt() to getString(R.string.bubble_listening)
            BubbleState.Processing -> 0xFF38BDF8.toInt() to getString(R.string.bubble_processing)
            BubbleState.Error -> 0xFFEF4444.toInt() to (lastError ?: "Error")
        }
        bubbleButton?.background?.mutate()?.setTint(color)
        statusLabel?.text = label
        if (state != BubbleState.Idle && panelExpanded) {
            panelExpanded = true
            panel?.visibility = View.VISIBLE
        }
    }

    private fun startDictation() {
        if (state != BubbleState.Idle) return
        try {
            recorder.start()
            setState(BubbleState.Recording)
            panelExpanded = true
            panel?.visibility = View.VISIBLE
            pumpJob = scope.launch(Dispatchers.IO) { recorder.pump() }
        } catch (e: Exception) {
            setState(BubbleState.Error, e.message ?: "Mic failed")
        }
    }

    private fun stopDictationAndProcess() {
        if (state != BubbleState.Recording) return
        pumpJob?.cancel()
        pumpJob = null
        setState(BubbleState.Processing)
        scope.launch {
            try {
                val wav = withContext(Dispatchers.IO) { recorder.stopToWav() }
                if (wav.size < 44 + 3200) {
                    setState(BubbleState.Error, getString(R.string.bubble_too_short))
                    recoverFromError()
                    return@launch
                }
                val settings = (application as FlowApp).settings.current()
                val client = FlowApiClient(settings.apiUrl, settings.apiKey)
                val result = withContext(Dispatchers.IO) {
                    client.dictate(wav, settings.language)
                }
                val text =
                    if (settings.correctEnglish) result.text
                    else (result.rawTranscript ?: result.text)
                val a11y = FlowAccessibilityService.instance
                if (a11y == null) copyFallback(text) else a11y.appendText(text)
                setState(BubbleState.Idle)
            } catch (e: Exception) {
                setState(BubbleState.Error, e.message ?: "Dictate failed")
                recoverFromError()
            }
        }
    }

    private fun runVibe() {
        if (state == BubbleState.Recording || state == BubbleState.Processing) return
        scope.launch {
            setState(BubbleState.Processing)
            try {
                val a11y = FlowAccessibilityService.instance
                    ?: throw IllegalStateException(getString(R.string.bubble_need_a11y))
                val selected = a11y.readFocusedText().trim()
                if (selected.isEmpty()) throw IllegalStateException(getString(R.string.bubble_need_text))
                val settings = (application as FlowApp).settings.current()
                val client = FlowApiClient(settings.apiUrl, settings.apiKey)
                val result = withContext(Dispatchers.IO) {
                    client.vibeText(
                        selectedText = selected,
                        projectContext = PromptAssets.loadProjectContext(this@BubbleOverlayService),
                        constitution = PromptAssets.loadConstitution(this@BubbleOverlayService),
                        skill = PromptAssets.loadVibeSkill(this@BubbleOverlayService),
                        language = settings.language,
                    )
                }
                a11y.insertOrReplaceText(result.text)
                setState(BubbleState.Idle)
            } catch (e: Exception) {
                setState(BubbleState.Error, e.message ?: "Vibe failed")
                recoverFromError()
            }
        }
    }

    private fun runGrammar() {
        if (state == BubbleState.Recording || state == BubbleState.Processing) return
        scope.launch {
            setState(BubbleState.Processing)
            try {
                val a11y = FlowAccessibilityService.instance
                    ?: throw IllegalStateException(getString(R.string.bubble_need_a11y))
                val selected = a11y.readFocusedText().trim()
                if (selected.isEmpty()) throw IllegalStateException(getString(R.string.bubble_need_text))
                val settings = (application as FlowApp).settings.current()
                val client = FlowApiClient(settings.apiUrl, settings.apiKey)
                val result = withContext(Dispatchers.IO) {
                    client.correctText(selected, settings.language)
                }
                a11y.insertOrReplaceText(result.text)
                setState(BubbleState.Idle)
            } catch (e: Exception) {
                setState(BubbleState.Error, e.message ?: "Grammar failed")
                recoverFromError()
            }
        }
    }

    private fun recoverFromError() {
        scope.launch {
            delay(3200)
            if (state == BubbleState.Error) setState(BubbleState.Idle)
        }
    }

    private fun copyFallback(text: String) {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard.setPrimaryClip(ClipData.newPlainText("Android Flow", text))
        Toast.makeText(this, getString(R.string.bubble_copied_a11y), Toast.LENGTH_LONG).show()
    }

    private fun buildNotification(): Notification {
        val channelId = "flow_bubble"
        val manager = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            manager.createNotificationChannel(
                NotificationChannel(
                    channelId,
                    getString(R.string.bubble_notification_channel),
                    NotificationManager.IMPORTANCE_LOW,
                ),
            )
        }
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, channelId)
            .setContentTitle(getString(R.string.bubble_notification_title))
            .setContentText(getString(R.string.bubble_notification_text))
            .setSmallIcon(R.drawable.ic_mic)
            .setContentIntent(open)
            .setOngoing(true)
            .build()
    }

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()

    companion object {
        const val NOTIFICATION_ID = 42
        const val ACTION_STOP = "com.efi.androidflow.STOP_BUBBLE"

        fun start(context: Context) {
            val intent = Intent(context, BubbleOverlayService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, BubbleOverlayService::class.java))
        }
    }
}
