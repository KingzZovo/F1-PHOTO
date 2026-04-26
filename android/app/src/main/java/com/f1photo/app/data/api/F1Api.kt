package com.f1photo.app.data.api

import okhttp3.MultipartBody
import okhttp3.RequestBody
import retrofit2.Response
import retrofit2.http.Body
import retrofit2.http.GET
import retrofit2.http.Multipart
import retrofit2.http.POST
import retrofit2.http.Part
import retrofit2.http.Path
import retrofit2.http.Query

/**
 * Thin Retrofit interface mirroring the routes used by the mobile client.
 * Full surface lives in server/src/api/*.rs.
 */
interface F1Api {
    @POST("/api/auth/login")
    suspend fun login(@Body body: LoginRequest): LoginResponse

    @POST("/api/auth/logout")
    suspend fun logout(): Response<Unit>

    @GET("/api/projects/{pid}/work_orders")
    suspend fun listWorkOrders(
        @Path("pid") projectId: String,
        @Query("q") q: String? = null,
        @Query("status") status: String? = null,
        @Query("page") page: Int = 1,
        @Query("page_size") pageSize: Int = 50,
    ): WorkOrderListResponse

    /**
     * Multipart upload. Server enforces:
     *   - file (required)
     *   - owner_type (required: person|tool|device|wo_raw)
     *   - one of wo_id (uuid) or wo_code
     *   - optional: owner_id, employee_no, sn, angle
     * Returns 202 Accepted.
     */
    @Multipart
    @POST("/api/projects/{pid}/photos")
    suspend fun uploadPhoto(
        @Path("pid") projectId: String,
        @Part file: MultipartBody.Part,
        @Part("owner_type") ownerType: RequestBody,
        @Part("wo_id") woId: RequestBody? = null,
        @Part("wo_code") woCode: RequestBody? = null,
        @Part("owner_id") ownerId: RequestBody? = null,
        @Part("employee_no") employeeNo: RequestBody? = null,
        @Part("sn") sn: RequestBody? = null,
        @Part("angle") angle: RequestBody? = null,
    ): Response<PhotoUploadResponse>
}
