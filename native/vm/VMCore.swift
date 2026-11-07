import Foundation
import Virtualization
import Darwin

struct VMError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

struct VMSettings: Codable {
    var cpuCount = 8
    var memoryBytes: UInt64 = 16 * 1024 * 1024 * 1024
    var diskBytes: UInt64 = 128 * 1024 * 1024 * 1024
    var macAddress: String
    var username = "agent"
    var password: String
    var installed = false
}

/// A private experiment directory contains the disk, identity and guest credentials.
/// A process lock prevents the installer and UI opening the same disk together.
@MainActor
final class VMCore: NSObject, @preconcurrency VZVirtualMachineDelegate {
    let directory: URL
    var machine: VZVirtualMachine?
    var settings: VMSettings?
    private var lockFD: Int32 = -1

    init(directory: URL) throws {
        self.directory = directory
        super.init()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700])
        lockFD = Darwin.open(path("runtime.lock").path, O_CREAT | O_RDWR, 0o600)
        guard lockFD >= 0, flock(lockFD, LOCK_EX | LOCK_NB) == 0 else {
            throw VMError(message: "This VM is already owned by another process.")
        }
    }

    deinit { if lockFD >= 0 { Darwin.close(lockFD) } }
    func path(_ name: String) -> URL { directory.appendingPathComponent(name) }
    func status(_ state: String, _ message: String = "") {
        let value: [String: Any] = ["state": state, "message": message, "pid": getpid(),
                                   "updated": Date().timeIntervalSince1970]
        if let data = try? JSONSerialization.data(withJSONObject: value, options: .prettyPrinted) {
            try? data.write(to: path("status.json"), options: .atomic)
        }
        fputs("VM: \(state) \(message)\n", stderr)
    }
    func saveSettings() throws {
        try JSONEncoder().encode(settings).write(to: path("config.json"), options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path("config.json").path)
    }
    func loadSettings() throws {
        settings = try JSONDecoder().decode(VMSettings.self, from: Data(contentsOf: path("config.json")))
    }

    func configure() throws -> VZVirtualMachine {
        guard let settings else { throw VMError(message: "VM settings are missing") }
        let config = VZVirtualMachineConfiguration()
        config.cpuCount = settings.cpuCount
        config.memorySize = settings.memoryBytes
        config.bootLoader = VZMacOSBootLoader()
        let platform = VZMacPlatformConfiguration()
        guard let model = VZMacHardwareModel(dataRepresentation: try Data(contentsOf: path("hardware-model.bin"))),
              let identifier = VZMacMachineIdentifier(dataRepresentation: try Data(contentsOf: path("machine-id.bin"))) else {
            throw VMError(message: "Invalid VM hardware identity")
        }
        guard model.isSupported else { throw VMError(message: "VM hardware model is unsupported on this Mac") }
        platform.hardwareModel = model
        platform.machineIdentifier = identifier
        platform.auxiliaryStorage = VZMacAuxiliaryStorage(contentsOf: path("auxiliary-storage.bin"))
        config.platform = platform
        let graphics = VZMacGraphicsDeviceConfiguration()
        graphics.displays = [VZMacGraphicsDisplayConfiguration(widthInPixels: 1920, heightInPixels: 1200, pixelsPerInch: 144)]
        config.graphicsDevices = [graphics]
        config.keyboards = [VZUSBKeyboardConfiguration()]
        config.pointingDevices = [VZUSBScreenCoordinatePointingDeviceConfiguration()]
        let disk = try VZDiskImageStorageDeviceAttachment(url: path("disk.img"), readOnly: false)
        config.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: disk)]
        let network = VZVirtioNetworkDeviceConfiguration()
        network.attachment = VZNATNetworkDeviceAttachment()
        guard let mac = VZMACAddress(string: settings.macAddress) else { throw VMError(message: "Invalid MAC address") }
        network.macAddress = mac
        config.networkDevices = [network]
        let share = VZVirtioFileSystemDeviceConfiguration(tag: VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag)
        for name in ["artifacts", "results"] {
            try FileManager.default.createDirectory(at: path(name), withIntermediateDirectories: true)
        }
        share.share = VZMultipleDirectoryShare(directories: [
            "artifacts": VZSharedDirectory(url: path("artifacts"), readOnly: true),
            "results": VZSharedDirectory(url: path("results"), readOnly: false)
        ])
        config.directorySharingDevices = [share]
        try config.validate()
        let vm = VZVirtualMachine(configuration: config)
        vm.delegate = self
        machine = vm
        return vm
    }

    func prepare() async throws {
        if FileManager.default.fileExists(atPath: path("config.json").path) {
            try loadSettings()
            guard settings?.installed != true else { status("ready", "Already installed"); return }
        }
        let ipsw = path("restore.ipsw")
        if !FileManager.default.fileExists(atPath: ipsw.path) {
            status("resolving", "Finding Apple's latest supported macOS restore image")
            let image = try await VZMacOSRestoreImage.latestSupported
            status("downloading", image.url.absoluteString)
            let partial = path("restore.ipsw.download")
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/curl")
            process.arguments = ["--fail", "--location", "--retry", "3", "--continue-at", "-", "--output", partial.path, image.url.absoluteString]
            try process.run()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else { throw VMError(message: "Restore image download failed") }
            try FileManager.default.moveItem(at: partial, to: ipsw)
        }
        let image = try await VZMacOSRestoreImage.image(from: ipsw)
        guard let requirements = image.mostFeaturefulSupportedConfiguration else {
            throw VMError(message: "Restore image is incompatible with this Mac")
        }
        if settings == nil {
            settings = VMSettings(macAddress: VZMACAddress.randomLocallyAdministered().string,
                                  password: UUID().uuidString + "aA1!")
            settings?.cpuCount = max(8, requirements.minimumSupportedCPUCount)
            settings?.memoryBytes = max(16 * 1024 * 1024 * 1024, requirements.minimumSupportedMemorySize)
            try requirements.hardwareModel.dataRepresentation.write(to: path("hardware-model.bin"))
            try VZMacMachineIdentifier().dataRepresentation.write(to: path("machine-id.bin"))
            _ = try VZMacAuxiliaryStorage(creatingStorageAt: path("auxiliary-storage.bin"), hardwareModel: requirements.hardwareModel, options: [])
            guard FileManager.default.createFile(atPath: path("disk.img").path, contents: nil) else {
                throw VMError(message: "Could not create VM disk")
            }
            let handle = try FileHandle(forWritingTo: path("disk.img"))
            guard let diskBytes = settings?.diskBytes else { throw VMError(message: "VM settings are missing") }
            try handle.truncate(atOffset: diskBytes)
            try handle.close()
            try saveSettings()
        }
        let vm = try configure()
        status("installing", "Installing macOS into the private VM disk")
        let installer = VZMacOSInstaller(virtualMachine: vm, restoringFromImageAt: ipsw)
        let observation = installer.progress.observe(\.fractionCompleted, options: [.new]) { progress, _ in
            fputs(String(format: "macOS installation: %.0f%%\n", progress.fractionCompleted * 100), stderr)
        }
        try await installer.install()
        observation.invalidate()
        settings?.installed = true
        try saveSettings()
        status("ready", "Installed; first boot will provision the agent account")
    }

    func start() async throws {
        try loadSettings()
        guard settings?.installed == true else { throw VMError(message: "Run scripts/vm/prepare first") }
        let vm = try configure()
        let options = VZMacOSVirtualMachineStartOptions()
        #if compiler(>=6.4)
        if #available(macOS 27.0, *), let settings {
            let provisioning = VZMacGuestProvisioningOptions()
            provisioning.fullName = "Herdr Test Agent"
            provisioning.username = settings.username
            provisioning.password = settings.password
            provisioning.logsInAutomatically = true
            provisioning.enablesRemoteLogin = true
            try options.setGuestProvisioning(provisioning)
        }
        #endif
        status("starting")
        try await vm.start(options: options)
        status("running", "Native display; \(settings?.cpuCount ?? 0) CPUs, \((settings?.memoryBytes ?? 0) / (1024 * 1024 * 1024)) GiB RAM")
    }

    func guestDidStop(_ virtualMachine: VZVirtualMachine) { status("stopped") }
    func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: Error) { status("error", error.localizedDescription) }
}
