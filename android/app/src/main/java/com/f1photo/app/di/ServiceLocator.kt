package com.f1photo.app.di

import android.content.Context
import androidx.room.Room
import com.f1photo.app.data.AuthStore
import com.f1photo.app.data.SettingsStore
import com.f1photo.app.data.UploadRepository
import com.f1photo.app.data.api.NetworkModule
import com.f1photo.app.data.db.F1Database
import com.f1photo.app.data.db.UploadQueueDao
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking

/**
 * Tiny manual DI container. App-wide singletons are created lazily once on
 * first access via `ServiceLocator.init`.
 */
object ServiceLocator {
    private lateinit var appContext: Context
    val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val authStore: AuthStore by lazy { AuthStore(appContext) }
    val settingsStore: SettingsStore by lazy { SettingsStore(appContext) }

    private val database: F1Database by lazy {
        Room.databaseBuilder(appContext, F1Database::class.java, "f1photo.db")
            .fallbackToDestructiveMigration()
            .build()
    }
    val uploadQueueDao: UploadQueueDao by lazy { database.uploadQueueDao() }
    val uploadRepository: UploadRepository by lazy { UploadRepository(appContext, uploadQueueDao) }

    val network: NetworkModule by lazy {
        val initial = runBlocking { settingsStore.baseUrlFlow.first() }
        NetworkModule(authStore, initial)
    }

    fun init(context: Context) {
        appContext = context.applicationContext
        // React to settings changes by feeding updated base URLs into NetworkModule.
        scope.launch {
            settingsStore.baseUrlFlow.collect { url -> network.setBaseUrl(url) }
        }
    }
}
