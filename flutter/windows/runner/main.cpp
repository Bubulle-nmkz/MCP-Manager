#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include "flutter_window.h"
#include "utils.h"

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  // Garantir une seule instance de MCP Manager : si l'application tourne déjà
  // (même masquée dans la zone de notification), on la réaffiche au premier plan.
  HANDLE single_instance_mutex =
      ::CreateMutexW(nullptr, TRUE, L"Local\\MCPManagerV3SingleInstanceMutex");
  if (single_instance_mutex != nullptr &&
      ::GetLastError() == ERROR_ALREADY_EXISTS) {
    HWND existing_hwnd = nullptr;
    ::EnumWindows(
        [](HWND hwnd, LPARAM lparam) -> BOOL {
          wchar_t class_name[256] = {0};
          if (::GetClassNameW(hwnd, class_name, 256) > 0 &&
              wcscmp(class_name, L"FLUTTER_RUNNER_WIN32_WINDOW") == 0) {
            wchar_t title[256] = {0};
            ::GetWindowTextW(hwnd, title, 256);
            if (wcsstr(title, L"MCP Manager") != nullptr ||
                wcsstr(title, L"mcp_hub") != nullptr) {
              *reinterpret_cast<HWND*>(lparam) = hwnd;
              return FALSE;
            }
          }
          return TRUE;
        },
        reinterpret_cast<LPARAM>(&existing_hwnd));

    if (!existing_hwnd) {
      existing_hwnd = ::FindWindowW(L"FLUTTER_RUNNER_WIN32_WINDOW", nullptr);
    }
    if (existing_hwnd) {
      DWORD target_pid = 0;
      DWORD target_tid = ::GetWindowThreadProcessId(existing_hwnd, &target_pid);
      if (target_pid != 0) {
        ::AllowSetForegroundWindow(target_pid);
      }
      DWORD current_tid = ::GetCurrentThreadId();
      if (target_tid != 0 && target_tid != current_tid) {
        ::AttachThreadInput(current_tid, target_tid, TRUE);
      }
      ::PostMessageW(existing_hwnd, WM_APP + 0x4D43, 0, 0);
      ::ShowWindow(existing_hwnd, SW_SHOW);
      if (::IsIconic(existing_hwnd)) {
        ::ShowWindow(existing_hwnd, SW_RESTORE);
      } else {
        ::ShowWindow(existing_hwnd, SW_SHOWNORMAL);
      }
      ::SetForegroundWindow(existing_hwnd);
      if (target_tid != 0 && target_tid != current_tid) {
        ::AttachThreadInput(current_tid, target_tid, FALSE);
      }
    }
    ::CloseHandle(single_instance_mutex);
    return EXIT_SUCCESS;
  }

  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1280, 720);
  if (!window.Create(L"MCP Manager", origin, size)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  return EXIT_SUCCESS;
}
