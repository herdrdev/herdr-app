#include "shell.cpp"
#include <shellapi.h>
#include <cassert>
#include <iostream>

class BackgroundFolder final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IPersistFolder2> {
public:
    PIDLIST_ABSOLUTE id = nullptr;
    ~BackgroundFolder() { CoTaskMemFree(id); }
    IFACEMETHODIMP GetClassID(CLSID*) override { return E_NOTIMPL; }
    IFACEMETHODIMP Initialize(PCIDLIST_ABSOLUTE) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetCurFolder(PIDLIST_ABSOLUTE* output) override {
        *output = ILCloneFull(id);
        return *output ? S_OK : E_OUTOFMEMORY;
    }
};

class BackgroundView final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IFolderView> {
public:
    ComPtr<IPersistFolder2> folder;
    IFACEMETHODIMP GetFolder(REFIID iid, void** output) override { return folder->QueryInterface(iid, output); }
    IFACEMETHODIMP GetCurrentViewMode(UINT*) override { return E_NOTIMPL; }
    IFACEMETHODIMP SetCurrentViewMode(UINT) override { return E_NOTIMPL; }
    IFACEMETHODIMP Item(int, PITEMID_CHILD*) override { return E_NOTIMPL; }
    IFACEMETHODIMP ItemCount(UINT, int*) override { return E_NOTIMPL; }
    IFACEMETHODIMP Items(UINT, REFIID, void**) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetSelectionMarkedItem(int*) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetFocusedItem(int*) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetItemPosition(PCUITEMID_CHILD, POINT*) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetSpacing(POINT*) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetDefaultSpacing(POINT*) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetAutoArrange() override { return E_NOTIMPL; }
    IFACEMETHODIMP SelectItem(int, DWORD) override { return E_NOTIMPL; }
    IFACEMETHODIMP SelectAndPositionItems(UINT, PCUITEMID_CHILD_ARRAY, POINT*, DWORD) override { return E_NOTIMPL; }
};

class BackgroundSite final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IServiceProvider> {
public:
    ComPtr<IFolderView> view;
    IFACEMETHODIMP QueryService(REFGUID service, REFIID iid, void** output) override {
        *output = nullptr;
        return service == SID_SFolderView ? view->QueryInterface(iid, output) : E_NOINTERFACE;
    }
};

class EmptySelection final : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IShellItemArray> {
public:
    DWORD count = 0;
    IFACEMETHODIMP GetCount(DWORD* output) override { *output = count; return S_OK; }
    IFACEMETHODIMP GetItemAt(DWORD, IShellItem**) override { return E_INVALIDARG; }
    IFACEMETHODIMP BindToHandler(IBindCtx*, REFGUID, REFIID, void**) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetPropertyStore(GETPROPERTYSTOREFLAGS, REFIID, void**) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetPropertyDescriptionList(REFPROPERTYKEY, REFIID, void**) override { return E_NOTIMPL; }
    IFACEMETHODIMP GetAttributes(SIATTRIBFLAGS, SFGAOF, SFGAOF*) override { return E_NOTIMPL; }
    IFACEMETHODIMP EnumItems(IEnumShellItems**) override { return E_NOTIMPL; }
};

int wmain() {
    assert(SUCCEEDED(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED)));
    for (const std::wstring value : {L"C:\\", L"C:\\Test Folder (ACME)", L"C:\\中文 O'Brien", L"a\\\"b", L""}) {
        const auto line = std::wstring(L"app open ") + quote(value);
        int count = 0;
        auto args = CommandLineToArgvW(line.c_str(), &count);
        assert(args && count == 3 && value == args[2]);
        LocalFree(args);
    }
    wchar_t temporary[MAX_PATH];
    assert(GetTempPathW(MAX_PATH, temporary));
    ComPtr<IShellItem> folder;
    assert(SUCCEEDED(SHCreateItemFromParsingName(temporary, nullptr, IID_PPV_ARGS(&folder))));
    ComPtr<IShellItemArray> selection;
    assert(SUCCEEDED(SHCreateShellItemArrayFromShellItem(folder.Get(), IID_PPV_ARGS(&selection))));
    std::wstring selected, background;
    assert(SUCCEEDED(folder_path(selection.Get(), nullptr, selected)));
    assert(SUCCEEDED(folder_path(nullptr, folder.Get(), background)) && selected == background);
    assert(FAILED(folder_path(nullptr, nullptr, background)));
    wchar_t executable[32768];
    assert(GetModuleFileNameW(nullptr, executable, 32768));
    ComPtr<IShellItem> file;
    assert(SUCCEEDED(SHCreateItemFromParsingName(executable, nullptr, IID_PPV_ARGS(&file))));
    assert(FAILED(folder_path(nullptr, file.Get(), background)));
    ComPtr<IShellItem> virtualFolder;
    assert(SUCCEEDED(SHCreateItemInKnownFolder(FOLDERID_ControlPanelFolder, 0, nullptr, IID_PPV_ARGS(&virtualFolder))));
    assert(FAILED(folder_path(nullptr, virtualFolder.Get(), background)));
    ComPtr<IClassFactory> factory;
    assert(DllGetClassObject(GUID_NULL, IID_PPV_ARGS(&factory)) == CLASS_E_CLASSNOTAVAILABLE);
    assert(SUCCEEDED(DllGetClassObject(CLSID_HerdrCommand, IID_PPV_ARGS(&factory))));
    ComPtr<IExplorerCommand> command;
    assert(SUCCEEDED(factory->CreateInstance(nullptr, IID_PPV_ARGS(&command))));
    EXPCMDSTATE state;
    assert(SUCCEEDED(command->GetState(selection.Get(), FALSE, &state)) && state == ECS_ENABLED);
    ComPtr<IObjectWithSite> site;
    assert(SUCCEEDED(command.As(&site)));
    assert(SUCCEEDED(site->SetSite(folder.Get())));
    ComPtr<IShellItem> returnedSite;
    assert(SUCCEEDED(site->GetSite(IID_PPV_ARGS(&returnedSite))) && returnedSite.Get() == folder.Get());
    returnedSite.Reset();
    assert(SUCCEEDED(site->SetSite(nullptr)));
    auto backgroundFolder = Make<BackgroundFolder>();
    assert(SUCCEEDED(SHGetIDListFromObject(folder.Get(), &backgroundFolder->id)));
    auto backgroundView = Make<BackgroundView>();
    backgroundView->folder = backgroundFolder;
    auto backgroundSite = Make<BackgroundSite>();
    backgroundSite->view = backgroundView;
    assert(SUCCEEDED(site->SetSite(backgroundSite.Get())));
    auto empty = Make<EmptySelection>();
    assert(SUCCEEDED(command->GetState(empty.Get(), FALSE, &state)) && state == ECS_ENABLED);
    assert(SUCCEEDED(command->GetState(nullptr, FALSE, &state)) && state == ECS_ENABLED);
    empty->count = 2;
    assert(SUCCEEDED(command->GetState(empty.Get(), FALSE, &state)) && state == ECS_HIDDEN);
    assert(DllCanUnloadNow() == S_FALSE);
    site.Reset(); command.Reset(); factory.Reset();
    assert(DllCanUnloadNow() == S_OK);
    virtualFolder.Reset(); file.Reset(); selection.Reset(); folder.Reset();
    CoUninitialize();
    std::cout << "Shell command path, quoting and COM checks passed.\n";
}
