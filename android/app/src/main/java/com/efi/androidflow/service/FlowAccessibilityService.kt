package com.efi.androidflow.service

import android.accessibilityservice.AccessibilityService
import android.os.Bundle
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo

/**
 * Reads the focused editable field and inserts processed text.
 * Insert is Accessibility-only; there is no clipboard fallback.
 */
class FlowAccessibilityService : AccessibilityService() {

    override fun onServiceConnected() {
        instance = this
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        // Focus tracking is on-demand via rootInActiveWindow when actions run.
    }

    override fun onInterrupt() = Unit

    override fun onDestroy() {
        if (instance === this) instance = null
        super.onDestroy()
    }

    fun readFocusedText(): String {
        val root = rootInActiveWindow ?: return ""
        val focused = findFocusedEditable(root) ?: return ""
        return focused.text?.toString().orEmpty()
    }

    fun insertOrReplaceText(text: String): Boolean {
        val root = rootInActiveWindow
        val focused = root?.let { findFocusedEditable(it) } ?: return false
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        }
        return focused.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
    }

    fun appendText(text: String): Boolean {
        val root = rootInActiveWindow
        val focused = root?.let { findFocusedEditable(it) } ?: return false
        val existing = focused.text?.toString().orEmpty()
        val joined = if (existing.isBlank()) text else "$existing$text"
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, joined)
        }
        return focused.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
    }

    private fun findFocusedEditable(node: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        if (node.isFocused && node.isEditable) return node
        for (i in 0 until node.childCount) {
            val child = node.getChild(i) ?: continue
            val found = findFocusedEditable(child)
            if (found != null) return found
        }
        return null
    }

    companion object {
        @Volatile
        var instance: FlowAccessibilityService? = null
            private set

        fun isEnabled(): Boolean = instance != null
    }
}
