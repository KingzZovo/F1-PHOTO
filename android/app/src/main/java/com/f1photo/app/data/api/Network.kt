package com.f1photo.app.data.api

import com.f1photo.app.data.AuthStore
import com.jakewharton.retrofit2.converter.kotlinx.serialization.asConverterFactory
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.json.Json
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.logging.HttpLoggingInterceptor
import retrofit2.Retrofit
import java.util.concurrent.TimeUnit

/**
 * Builds Retrofit clients keyed by base URL. Re-creates the underlying client
 * when the base URL changes (settings screen) so callers always observe the
 * latest server target via `apiFlow`.
 *
 * Auth interceptor pulls the current JWT from `AuthStore` on every request and
 * adds Bearer header when present.
 */
class NetworkModule(
    private val authStore: AuthStore,
    initialBaseUrl: String,
) {
    private val json = Json {
        ignoreUnknownKeys = true
        explicitNulls = false
        encodeDefaults = false
    }

    private val authInterceptor = okhttp3.Interceptor { chain ->
        val token = authStore.token
        val request = if (!token.isNullOrBlank()) {
            chain.request().newBuilder()
                .header("Authorization", "Bearer $token")
                .build()
        } else chain.request()
        chain.proceed(request)
    }

    private val logging = HttpLoggingInterceptor().apply {
        level = HttpLoggingInterceptor.Level.BASIC
    }

    private val httpClient: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .writeTimeout(120, TimeUnit.SECONDS)
        .addInterceptor(authInterceptor)
        .addInterceptor(logging)
        .build()

    private val baseUrlState = MutableStateFlow(initialBaseUrl)
    val baseUrl: StateFlow<String> = baseUrlState.asStateFlow()

    private var cachedBaseUrl: String = initialBaseUrl
    private var cachedApi: F1Api = build(initialBaseUrl)

    @Synchronized
    fun api(): F1Api {
        val current = baseUrlState.value
        if (current != cachedBaseUrl) {
            cachedBaseUrl = current
            cachedApi = build(current)
        }
        return cachedApi
    }

    fun apiFlow(): Flow<F1Api> = kotlinx.coroutines.flow.flow {
        baseUrlState.collect { _ -> emit(api()) }
    }

    fun setBaseUrl(value: String) {
        baseUrlState.value = value
    }

    private fun build(baseUrl: String): F1Api {
        val normalized = if (baseUrl.endsWith("/")) baseUrl else "$baseUrl/"
        return Retrofit.Builder()
            .baseUrl(normalized)
            .client(httpClient)
            .addConverterFactory(json.asConverterFactory("application/json".toMediaType()))
            .build()
            .create(F1Api::class.java)
    }
}
