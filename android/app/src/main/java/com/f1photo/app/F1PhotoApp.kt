package com.f1photo.app

import android.app.Application
import androidx.work.Configuration
import com.f1photo.app.di.ServiceLocator

class F1PhotoApp : Application(), Configuration.Provider {
    override fun onCreate() {
        super.onCreate()
        ServiceLocator.init(this)
    }

    override val workManagerConfiguration: Configuration
        get() = Configuration.Builder()
            .setMinimumLoggingLevel(android.util.Log.INFO)
            .build()
}
