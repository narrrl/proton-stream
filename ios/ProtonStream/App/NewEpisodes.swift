import BackgroundTasks
import Foundation
import UserNotifications

/// Look at the shares while the app is away, and say when a show gained
/// episodes — what `NewEpisodesWorker` does on Android.
///
/// A processing task rather than an app refresh: a crawl of a large share
/// takes minutes, and an app refresh gets about thirty seconds. iOS runs it
/// when it sees fit, mostly overnight on a charger, so it is a best effort
/// where Android's is a schedule.
enum NewEpisodes {
    static let identifier = "io.narl.protonstream.new-episodes"
    private static let interval: TimeInterval = 6 * 60 * 60

    /// At launch, before the app finishes launching: iOS refuses a handler
    /// registered later.
    static func register() {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) { task in
            guard let task = task as? BGProcessingTask else { return }
            run(task)
        }
    }

    /// Keep the next look scheduled, or cancel it when the viewer turned it off.
    static func schedule() {
        guard SettingsStore().newEpisodeNotifications else {
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: identifier)
            return
        }
        let request = BGProcessingTaskRequest(identifier: identifier)
        request.requiresNetworkConnectivity = true
        request.earliestBeginDate = Date(timeIntervalSinceNow: interval)
        try? BGTaskScheduler.shared.submit(request)
    }

    /// Ask once, when the viewer has the setting on; iOS remembers the answer.
    static func requestPermission() {
        guard SettingsStore().newEpisodeNotifications else { return }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    private static func run(_ task: BGProcessingTask) {
        schedule()
        let work = Task {
            let news = (try? await NativeRuntime.engine().crawlForNews()) ?? []
            await post(news)
            task.setTaskCompleted(success: !Task.isCancelled)
        }
        task.expirationHandler = {
            work.cancel()
            Task { try? await NativeRuntime.engine().stopCrawl() }
        }
    }

    private static func post(_ news: [NewEpisodesRecord]) async {
        let center = UNUserNotificationCenter.current()
        guard await center.notificationSettings().authorizationStatus == .authorized else { return }
        for item in news {
            let content = UNMutableNotificationContent()
            content.title = item.title
            content.body = describe(item)
            content.threadIdentifier = "new-episodes"
            // Keyed by the show, so a second look's news replaces the first.
            let request = UNNotificationRequest(identifier: "new-episodes-\(item.titleKey)", content: content, trigger: nil)
            try? await center.add(request)
        }
    }

    /// What one show's notification says: "S01E04 and 2 more are new".
    static func describe(_ news: NewEpisodesRecord) -> String {
        let more = Int(news.count) - 1
        guard let first = news.first else {
            return news.count == 1 ? "1 new episode" : "\(news.count) new episodes"
        }
        switch more {
        case 0: return "\(first) is new"
        case 1: return "\(first) and 1 more are new"
        default: return "\(first) and \(more) more are new"
        }
    }
}
