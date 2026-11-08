#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shlobj.h>
#include <shobjidl.h>
#include <shlwapi.h>
#include <wrl.h>
#include <atomic>
#include <filesystem>
#include <memory>
#include <string>
#include <vector>

using Microsoft::WRL::ComPtr;
using Microsoft::WRL::Make;
using Microsoft::WRL::RuntimeClass;
using Microsoft::WRL::RuntimeClassFlags;
using Microsoft::WRL::ClassicCom;

// Keep this identity in sync with AppxManifest.xml.in.
const CLSID CLSID_HerdrCommand = {0x2976d73e, 0xbc10, 0x4bae, {0xa6, 0x8b, 0x04, 0x10, 0x48, 0xf2, 0x54, 0x98}};
static HMODULE module;
static std::atomic<long> objects{0};

static std::wstring quote(const std::wstring& output) {
    std::wstring result = L"\"";
    size_t slashes = 0;
    for (wchar_t ch : output) {
        if (ch == L'\\') { ++slashes; continue; }
        result.append(slashes * (ch == L'\"' ? 2 : 1), L'\\');
        if (ch == L'\"') result += L'\\';
        result += ch;
        slashes = 0;
    }
    result.append(slashes * 2, L'\\');
    return result + L'\"';
}

static HRESULT folder_path(IShellItemArray* selection, IShellItem* background, std::wstring& path) {
    ComPtr<IShellItem> item;
    if (selection) {
        DWORD count = 0;
        HRESULT hr = selection->GetCount(&count);
        if (FAILED(hr)) return hr;
        if (count != 1) return E_INVALIDARG;
        hr = selection->GetItemAt(0, &item);
        if (FAILED(hr)) return hr;
    } else {
        item = background;
    }
    if (!item) return E_INVALIDARG;
    SFGAOF attributes = 0;
    HRESULT hr = item->GetAttributes(SFGAO_FOLDER | SFGAO_FILESYSTEM, &attributes);
    if (FAILED(hr)) return hr;
    if ((attributes & (SFGAO_FOLDER | SFGAO_FILESYSTEM)) != (SFGAO_FOLDER | SFGAO_FILESYSTEM)) return E_INVALIDARG;
    PWSTR name = nullptr;
    hr = item->GetDisplayName(SIGDN_FILESYSPATH, &name);
    const std::unique_ptr<wchar_t, decltype(&CoTaskMemFree)> allocated(name, CoTaskMemFree);
    if (SUCCEEDED(hr)) path = name;
    return hr;
}

static std::filesystem::path app_path() {
    std::vector<wchar_t> name(32768);
    DWORD length = GetModuleFileNameW(module, name.data(), static_cast<DWORD>(name.size()));
    if (!length || length == name.size()) return {};
    // Programs/herdr-app-shell/<version>/herdr-explorer.dll lives outside app swaps.
    return std::filesystem::path(name.data()).parent_path().parent_path().parent_path() / L"herdr-app" / L"herdr-app.exe";
}

class Command final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IExplorerCommand, IObjectWithSite> {
    ComPtr<IUnknown> site;

    HRESULT path(IShellItemArray* items, std::wstring& result) {
        if (items) {
            DWORD count = 0;
            HRESULT hr = items->GetCount(&count);
            if (FAILED(hr)) return hr;
            if (count) return folder_path(items, nullptr, result);
        }
        ComPtr<IServiceProvider> provider;
        ComPtr<IShellBrowser> browser;
        ComPtr<IShellView> view;
        ComPtr<IFolderView> folderView;
        ComPtr<IPersistFolder2> folder;
        if (!site) return E_INVALIDARG;
        HRESULT hr = site.As(&provider);
        if (FAILED(hr)) return hr;
        hr = provider->QueryService(SID_SFolderView, IID_PPV_ARGS(&folderView));
        if (FAILED(hr)) {
            hr = provider->QueryService(SID_STopLevelBrowser, IID_PPV_ARGS(&browser));
            if (FAILED(hr)) return hr;
            hr = browser->QueryActiveShellView(&view);
            if (FAILED(hr)) return hr;
            hr = view.As(&folderView);
            if (FAILED(hr)) return hr;
        }
        hr = folderView->GetFolder(IID_PPV_ARGS(&folder));
        if (FAILED(hr)) return hr;
        PIDLIST_ABSOLUTE pidl = nullptr;
        hr = folder->GetCurFolder(&pidl);
        if (FAILED(hr)) return hr;
        ComPtr<IShellItem> item;
        hr = SHCreateItemFromIDList(pidl, IID_PPV_ARGS(&item));
        CoTaskMemFree(pidl);
        return SUCCEEDED(hr) ? folder_path(nullptr, item.Get(), result) : hr;
    }

public:
    Command() { ++objects; }
    ~Command() { --objects; }
    IFACEMETHODIMP SetSite(IUnknown* output) override { site = output; return S_OK; }
    IFACEMETHODIMP GetSite(REFIID iid, void** output) override {
        if (!output) return E_POINTER;
        *output = nullptr;
        return site ? site->QueryInterface(iid, output) : E_FAIL;
    }
    IFACEMETHODIMP GetTitle(IShellItemArray*, PWSTR* output) override {
        return SHStrDupW(PRIMARYLANGID(GetUserDefaultUILanguage()) == LANG_RUSSIAN ? L"Открыть в Herdr" : L"Open in Herdr", output);
    }
    IFACEMETHODIMP GetIcon(IShellItemArray*, PWSTR* output) override try {
        *output = nullptr;
        const auto app = app_path();
        if (app.empty()) { *output = nullptr; return E_FAIL; }
        return SHStrDupW((app.wstring() + L",0").c_str(), output);
    } catch (const std::bad_alloc&) { return E_OUTOFMEMORY; } catch (...) { return E_FAIL; }
    IFACEMETHODIMP GetToolTip(IShellItemArray*, PWSTR* output) override { *output = nullptr; return E_NOTIMPL; }
    IFACEMETHODIMP GetCanonicalName(GUID* output) override { *output = CLSID_HerdrCommand; return S_OK; }
    IFACEMETHODIMP GetState(IShellItemArray* items, BOOL, EXPCMDSTATE* output) override try {
        *output = ECS_HIDDEN;
        std::wstring directory;
        *output = SUCCEEDED(path(items, directory)) ? ECS_ENABLED : ECS_HIDDEN;
        return S_OK;
    } catch (const std::bad_alloc&) { return E_OUTOFMEMORY; } catch (...) { return E_FAIL; }
    IFACEMETHODIMP Invoke(IShellItemArray* items, IBindCtx*) override try {
        std::wstring directory;
        HRESULT hr = path(items, directory);
        if (FAILED(hr)) return hr;
        const auto app = app_path();
        if (app.empty()) return E_FAIL;
        auto command = quote(app.wstring()) + L" open --explorer " + quote(directory);
        STARTUPINFOW startup{sizeof(startup)};
        PROCESS_INFORMATION process{};
        if (!CreateProcessW(app.c_str(), command.data(), nullptr, nullptr, FALSE, 0, nullptr, directory.c_str(), &startup, &process)) return HRESULT_FROM_WIN32(GetLastError());
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        return S_OK;
    } catch (const std::bad_alloc&) { return E_OUTOFMEMORY; } catch (...) { return E_FAIL; }
    IFACEMETHODIMP GetFlags(EXPCMDFLAGS* output) override { *output = ECF_DEFAULT; return S_OK; }
    IFACEMETHODIMP EnumSubCommands(IEnumExplorerCommand** output) override { *output = nullptr; return E_NOTIMPL; }
};

class Factory final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IClassFactory> {
public:
    Factory() { ++objects; }
    ~Factory() { --objects; }
    IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID iid, void** output) override {
        if (!output) return E_POINTER;
        *output = nullptr;
        if (outer) return CLASS_E_NOAGGREGATION;
        auto command = Make<Command>();
        return command ? command->QueryInterface(iid, output) : E_OUTOFMEMORY;
    }
    IFACEMETHODIMP LockServer(BOOL lock) override { if (lock) ++objects; else --objects; return S_OK; }
};

STDAPI DllGetClassObject(REFCLSID clsid, REFIID iid, void** output) {
    if (!output) return E_POINTER;
    *output = nullptr;
    if (clsid != CLSID_HerdrCommand) return CLASS_E_CLASSNOTAVAILABLE;
    auto factory = Make<Factory>();
    return factory ? factory->QueryInterface(iid, output) : E_OUTOFMEMORY;
}

STDAPI DllCanUnloadNow() { return objects == 0 ? S_OK : S_FALSE; }

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) module = instance;
    return TRUE;
}
