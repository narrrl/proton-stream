package io.narl.protonstream.ui

import android.content.res.Configuration
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.input.InputMode
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.platform.LocalInputModeManager
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performKeyInput
import androidx.compose.ui.test.pressKey
import androidx.compose.ui.test.requestFocus
import androidx.test.core.app.ApplicationProvider
import androidx.work.testing.WorkManagerTestInitHelper
import com.github.takahirom.roborazzi.RobolectricDeviceQualifiers
import io.narl.protonstream.ui.theme.AppearanceState
import io.narl.protonstream.ui.theme.ProtonFallback
import io.narl.protonstream.ui.theme.ProtonStreamTheme
import io.narl.protonstream.ui.tv.TvHome
import io.narl.protonstream.ui.tv.TvTheme
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The app driven by a remote: what a D-pad and a Menu key can reach. */
@RunWith(RobolectricTestRunner::class)
@Config(qualifiers = RobolectricDeviceQualifiers.Television1080p)
class TvTest {
    @get:Rule val compose = createComposeRule()

    @Before
    fun setUp() {
        WorkManagerTestInitHelper.initializeTestWorkManager(ApplicationProvider.getApplicationContext())
        AppearanceState.apply(ProtonFallback)
    }

    @Test
    fun `a television is told apart from a phone by its ui mode`() {
        val television = Configuration().apply {
            uiMode = Configuration.UI_MODE_TYPE_TELEVISION or Configuration.UI_MODE_NIGHT_YES
        }
        val phone = Configuration().apply {
            uiMode = Configuration.UI_MODE_TYPE_NORMAL or Configuration.UI_MODE_NIGHT_YES
        }
        assertTrue(isTelevision(television))
        assertFalse(isTelevision(phone))
    }

    @OptIn(ExperimentalComposeUiApi::class)
    @Test
    fun `the menu key opens what a long press on a tile opens`() {
        compose.setContent {
            // As after the first press of a remote's key: Compose only lets a
            // clickable take the focus once keys are driving.
            val modes = LocalInputModeManager.current
            LaunchedEffect(Unit) { modes.requestInputMode(InputMode.Keyboard) }
            ProtonStreamTheme {
                CompositionLocalProvider(LocalArtworkLoading provides false) {
                    Surface(color = MaterialTheme.colorScheme.background) {
                        LibraryScreen(
                            ScreenshotTest.MATCHED, {}, { _, _ -> }, {}, {}, { _, _ -> }, { _, _ -> }, PaddingValues(),
                        )
                    }
                }
            }
        }
        val tile = compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsActions.OnLongClick)).onFirst()
        tile.requestFocus()
        tile.performKeyInput { pressKey(Key.Menu) }
        compose.onAllNodes(hasText("Remove from Continue watching") or hasText("Download all"))
            .onFirst()
            .assertExists()
    }

    @OptIn(ExperimentalComposeUiApi::class)
    @Test
    fun `the menu key on a card of the television's home opens its menu`() {
        compose.setContent {
            val modes = LocalInputModeManager.current
            LaunchedEffect(Unit) { modes.requestInputMode(InputMode.Keyboard) }
            ProtonStreamTheme {
                CompositionLocalProvider(LocalArtworkLoading provides false) {
                    TvTheme {
                        TvHome(ScreenshotTest.MATCHED, ScreenshotTest.TV_ACTIONS, { LazyListState() }, LazyListState(), null, {})
                    }
                }
            }
        }
        val card = compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsActions.OnLongClick)).onFirst()
        card.requestFocus()
        card.performKeyInput { pressKey(Key.Menu) }
        compose.onNode(hasText("Open")).assertExists()
    }
}
