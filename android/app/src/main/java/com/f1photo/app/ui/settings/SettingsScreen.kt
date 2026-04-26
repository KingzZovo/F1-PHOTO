package com.f1photo.app.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.f1photo.app.di.ServiceLocator
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(onBack: () -> Unit) {
    val store = ServiceLocator.settingsStore
    val baseUrl by store.baseUrlFlow.collectAsState(initial = "")
    val projectId by store.projectIdFlow.collectAsState(initial = "")
    val scope = rememberCoroutineScope()

    var localUrl by remember(baseUrl) { mutableStateOf(baseUrl) }
    var localProject by remember(projectId) { mutableStateOf(projectId) }
    var saved by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("设置") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.Default.ArrowBack, contentDescription = "返回")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("服务器地址")
            OutlinedTextField(
                value = localUrl,
                onValueChange = { localUrl = it; saved = false },
                singleLine = true,
                placeholder = { Text("http://10.0.2.2:18080") },
                modifier = Modifier.fillMaxWidth(),
            )
            Text("项目 ID")
            OutlinedTextField(
                value = localProject,
                onValueChange = { localProject = it; saved = false },
                singleLine = true,
                placeholder = { Text("00000000-0000-0000-0000-000000000001") },
                modifier = Modifier.fillMaxWidth(),
            )
            Button(
                onClick = {
                    scope.launch {
                        store.setBaseUrl(localUrl)
                        store.setProjectId(localProject)
                        ServiceLocator.network.setBaseUrl(localUrl)
                        saved = true
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("保存")
            }
            if (saved) Text("已保存", color = MaterialTheme.colorScheme.primary)
        }
    }
}
