package com.recall.app.ui

import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Storage
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.navigation.NavDestination.Companion.hierarchy
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import com.recall.app.RecallRepository

private data class TopLevelDest(val route: String, val label: String, val icon: ImageVector)

private val topLevelDestinations = listOf(
    TopLevelDest("feed", "Feed", Icons.Filled.Home),
    TopLevelDest("search", "Search", Icons.Filled.Search),
    TopLevelDest("actions", "Actions", Icons.Filled.CheckCircle),
    TopLevelDest("storage", "Storage", Icons.Filled.Storage),
    TopLevelDest("soak", "Soak", Icons.Filled.Settings),
)

/** Bottom-nav host: Feed / Search / Actions / Storage / Soak, plus a Detail screen pushed on top. */
@Composable
fun RecallNavHost(repository: RecallRepository, capturePermissionsGranted: Boolean) {
    val navController = rememberNavController()

    Scaffold(
        bottomBar = {
            val backStackEntry by navController.currentBackStackEntryAsState()
            val currentDestination = backStackEntry?.destination
            NavigationBar {
                topLevelDestinations.forEach { dest ->
                    NavigationBarItem(
                        selected = currentDestination?.hierarchy?.any { it.route == dest.route } == true,
                        onClick = {
                            navController.navigate(dest.route) {
                                popUpTo(navController.graph.findStartDestination().id) { saveState = true }
                                launchSingleTop = true
                                restoreState = true
                            }
                        },
                        icon = { Icon(dest.icon, contentDescription = dest.label) },
                        label = { Text(dest.label) },
                    )
                }
            }
        },
    ) { padding ->
        NavHost(
            navController = navController,
            startDestination = "feed",
            modifier = Modifier.padding(padding),
        ) {
            composable("feed") {
                FeedScreen(
                    repository = repository,
                    capturePermissionsGranted = capturePermissionsGranted,
                    onOpenConversation = { id -> navController.navigate("detail/$id") },
                )
            }
            composable("search") {
                SearchScreen(
                    repository = repository,
                    onOpenConversation = { id -> navController.navigate("detail/$id") },
                )
            }
            composable("actions") { ActionItemsScreen(repository) }
            composable("storage") { StorageScreen(repository) }
            composable("soak") { SoakScreen(repository) }
            composable(
                route = "detail/{id}",
                arguments = listOf(navArgument("id") { type = NavType.LongType }),
            ) { backStackEntry ->
                val id = backStackEntry.arguments?.getLong("id") ?: return@composable
                DetailScreen(
                    repository = repository,
                    conversationId = id,
                    onBack = { navController.popBackStack() },
                )
            }
        }
    }
}
