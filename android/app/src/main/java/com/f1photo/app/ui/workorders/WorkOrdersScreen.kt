package com.f1photo.app.ui.workorders

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ExitToApp
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Upload
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
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
import androidx.compose.ui.unit.dp
import com.f1photo.app.data.api.WorkOrder
import com.f1photo.app.di.ServiceLocator
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun WorkOrdersScreen(
    onOpenUpload: (String) -> Unit,
    onOpenQueue: () -> Unit,
    onOpenSettings: () -> Unit,
    onLogout: () -> Unit,
) {
    val network = ServiceLocator.network
    val settings = ServiceLocator.settingsStore
    val auth = ServiceLocator.authStore
    val scope = rememberCoroutineScope()
    val pendingCount by ServiceLocator.uploadRepository.pendingCount.collectAsState(initial = 0)
    val projectId by settings.projectIdFlow.collectAsState(initial = "")

    var loading by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var rows by remember { mutableStateOf<List<WorkOrder>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }

    suspend fun reload() {
        if (projectId.isBlank()) return
        loading = true
        error = null
        runCatching {
            network.api().listWorkOrders(
                projectId = projectId,
                q = query.trim().ifEmpty { null },
                page = 1,
                pageSize = 50,
            )
        }.onSuccess { rows = it.data }
            .onFailure { error = it.message ?: "加载失败" }
        loading = false
    }

    LaunchedEffect(projectId) {
        if (projectId.isNotBlank()) reload()
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("工单") },
                actions = {
                    IconButton(onClick = onOpenQueue) {
                        Icon(Icons.Default.Upload, contentDescription = "上传队列")
                    }
                    IconButton(onClick = { scope.launch { reload() } }) {
                        Icon(Icons.Default.Refresh, contentDescription = "刷新")
                    }
                    IconButton(onClick = onOpenSettings) {
                        Icon(Icons.Default.Settings, contentDescription = "设置")
                    }
                    IconButton(onClick = {
                        auth.clear()
                        onLogout()
                    }) {
                        Icon(Icons.Default.ExitToApp, contentDescription = "退出")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 16.dp),
        ) {
            if (pendingCount > 0) {
                TextButton(onClick = onOpenQueue, modifier = Modifier.padding(top = 8.dp)) {
                    Text("上传队列待处理 $pendingCount 张")
                }
            }
            Row(modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    label = { Text("搜索工单号/标题") },
                    singleLine = true,
                    modifier = Modifier.weight(1f),
                )
                Spacer(Modifier.height(8.dp))
                IconButton(onClick = { scope.launch { reload() } }) {
                    Icon(Icons.Default.Refresh, contentDescription = "搜索")
                }
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }

            if (loading) {
                Box(modifier = Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }
            }

            LazyColumn(
                modifier = Modifier.fillMaxSize(),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(rows, key = { it.id }) { wo -> WorkOrderRow(wo, onClick = { onOpenUpload(wo.id) }) }
            }
        }
    }
}

@Composable
private fun WorkOrderRow(wo: WorkOrder, onClick: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        elevation = CardDefaults.cardElevation(defaultElevation = 1.dp),
        onClick = onClick,
    ) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(wo.code, style = MaterialTheme.typography.titleMedium)
            wo.title?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
            Text("状态：${wo.status}", style = MaterialTheme.typography.bodySmall)
        }
    }
}
