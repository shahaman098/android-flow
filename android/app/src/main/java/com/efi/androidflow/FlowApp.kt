package com.efi.androidflow

import android.app.Application
import com.efi.androidflow.data.FlowLanguages
import com.efi.androidflow.data.SettingsRepository
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

class FlowApp : Application() {
    lateinit var settings: SettingsRepository
        private set

    private val appScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    override fun onCreate() {
        super.onCreate()
        settings = SettingsRepository(this)
        appScope.launch {
            settings.settingsFlow
                .map { FlowLanguages.normalize(it.language) }
                .distinctUntilChanged()
                .collect { FlowLanguages.applyAppLocale(it) }
        }
    }
}
