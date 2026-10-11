// SPDX-License-Identifier: GPL-3.0-or-later
//! Application-manager object hierarchy. Unsupported controller methods fail explicitly.

use crate::{
    HorizonProfileId, HorizonService,
    error::{HorizonError, HorizonResult},
    ipc::{ReturnedObject, ServiceRequest, ServiceResponse},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppletObject {
    ApplicationProxy,
    CommonStateGetter,
    SelfController,
    WindowController,
    AudioController,
    DisplayController,
    ProcessWindingController,
    LibraryAppletCreator,
    ApplicationFunctions,
    DebugFunctions,
}

impl AppletObject {
    pub fn name(self) -> &'static str {
        match self {
            Self::ApplicationProxy => "IApplicationProxy",
            Self::CommonStateGetter => "ICommonStateGetter",
            Self::SelfController => "ISelfController",
            Self::WindowController => "IWindowController",
            Self::AudioController => "IAudioController",
            Self::DisplayController => "IDisplayController",
            Self::ProcessWindingController => "IProcessWindingController",
            Self::LibraryAppletCreator => "ILibraryAppletCreator",
            Self::ApplicationFunctions => "IApplicationFunctions",
            Self::DebugFunctions => "IDebugFunctions",
        }
    }
}

#[derive(Debug)]
pub struct AppletService;

#[derive(Debug, Clone)]
pub struct ApplicationState {
    /// Application-selected termination result, separate from CMIF/SVC outcomes.
    /// Setting it does not terminate the process; it survives normal process exit.
    pub terminate_result: u32,
    pub focus_handling: [bool; 3],
    pub out_of_focus_suspending: bool,
    pub operation_mode_notifications: bool,
    pub performance_mode_notifications: bool,
    pub restart_messages: bool,
    pub screenshot_permission: u32,
    pub resource_user_id: u64,
    pub window_visible: bool,
    pub gpu_time_slice: i64,
    pub notification_event: crate::event::KernelEvent,
    messages: std::collections::VecDeque<u32>,
    pub focus_state: u8,
}

impl ApplicationState {
    pub fn new(resource_user_id: u64) -> Self {
        let mut notification_event = crate::event::KernelEvent::new(resource_user_id, false);
        notification_event.signal();
        Self {
            terminate_result: 0,
            focus_handling: [false; 3],
            out_of_focus_suspending: false,
            operation_mode_notifications: false,
            performance_mode_notifications: false,
            restart_messages: false,
            screenshot_permission: 0,
            resource_user_id,
            window_visible: true,
            gpu_time_slice: 0,
            notification_event,
            // A launched diagnostic application starts in focus, with a pending
            // notification. Reading focus does not consume the notification.
            messages: std::collections::VecDeque::from([15]),
            focus_state: 1,
        }
    }
}

impl HorizonService for AppletService {
    fn name(&self) -> &'static str {
        "appletOE"
    }

    fn call(
        &mut self,
        _: &HorizonProfileId,
        request: &ServiceRequest,
    ) -> HorizonResult<ServiceResponse> {
        if request.command_id != 0 {
            return Err(unknown("appletOE", request.command_id));
        }
        if request.raw_payload.get(..8) != Some(&[0u8; 8]) {
            return Err(HorizonError::IpcProtocol(
                "OpenApplicationProxy requires a zero reserved u64".into(),
            ));
        }
        // The kernel validates the process copy-handle before reaching this callback.
        object_response(AppletObject::ApplicationProxy)
    }
}

fn unknown(service: &str, command_id: u32) -> HorizonError {
    HorizonError::UnknownCommand {
        service: service.into(),
        command_id,
    }
}

fn boolean(request: &ServiceRequest, offset: usize) -> HorizonResult<bool> {
    match request.raw_payload.get(offset) {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(HorizonError::IpcProtocol(
            "self-controller requires a boolean".into(),
        )),
    }
}

fn object_response(object: AppletObject) -> HorizonResult<ServiceResponse> {
    let mut response = ServiceResponse::success();
    response.objects.push(ReturnedObject::Applet(object));
    Ok(response)
}

pub fn dispatch_with_display(
    object: AppletObject,
    state: &mut ApplicationState,
    display: &mut crate::display::DisplayManager,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    if object == AppletObject::SelfController && request.command_id == 40 {
        if !request.buffers.is_empty() {
            return Err(HorizonError::IpcProtocol(
                "CreateManagedDisplayLayer takes no buffers".into(),
            ));
        }
        let layer = display.create_managed_layer(state.resource_user_id)?;
        let mut response = ServiceResponse::success();
        response.raw_payload = layer.id.to_le_bytes().to_vec();
        return Ok(response);
    }
    dispatch(object, state, request)
}

pub fn dispatch(
    object: AppletObject,
    state: &mut ApplicationState,
    request: &ServiceRequest,
) -> HorizonResult<ServiceResponse> {
    if object == AppletObject::ApplicationFunctions {
        if request.command_id != 22 {
            return Err(unknown(object.name(), request.command_id));
        }
        let bytes = request.raw_payload.get(..4).ok_or_else(|| {
            HorizonError::IpcProtocol("SetTerminateResult requires a u32 result".into())
        })?;
        state.terminate_result = u32::from_le_bytes(bytes.try_into().unwrap());
        return Ok(ServiceResponse::success());
    }
    if object == AppletObject::SelfController {
        match request.command_id {
            10 => {
                let bytes = request.raw_payload.get(..4).ok_or_else(|| {
                    HorizonError::IpcProtocol("screenshot permission requires an s32".into())
                })?;
                let permission = u32::from_le_bytes(bytes.try_into().unwrap());
                if permission > 2 {
                    return Err(HorizonError::IpcProtocol(
                        "invalid screenshot permission".into(),
                    ));
                }
                state.screenshot_permission = permission;
            }
            11 => state.operation_mode_notifications = boolean(request, 0)?,
            12 => state.performance_mode_notifications = boolean(request, 0)?,
            13 => {
                // Validate all three adjacent ABI bytes before mutating shared policy.
                state.focus_handling = [
                    boolean(request, 0)?,
                    boolean(request, 1)?,
                    boolean(request, 2)?,
                ];
            }
            14 => state.restart_messages = boolean(request, 0)?,
            16 => state.out_of_focus_suspending = boolean(request, 0)?,
            command_id => return Err(unknown(object.name(), command_id)),
        }
        // Retained policy is consumed by future focus/suspend transitions. This does
        // not suspend the host or fabricate a display/capture implementation.
        return Ok(ServiceResponse::success());
    }
    if object == AppletObject::CommonStateGetter {
        let mut response = ServiceResponse::success();
        match request.command_id {
            0 => response.objects.push(ReturnedObject::AppletMessageEvent),
            1 => match state.messages.pop_front() {
                Some(message) => {
                    response.raw_payload = message.to_le_bytes().to_vec();
                    if state.messages.is_empty() {
                        state.notification_event.clear();
                    }
                }
                None => response.result_code = 0x680,
            },
            // Explicit diagnostic handheld policy, consistent with the APM manager.
            5 => response.raw_payload = vec![0],
            6 => response.raw_payload = 0u32.to_le_bytes().to_vec(),
            9 => response.raw_payload = vec![state.focus_state],
            command_id => return Err(unknown(object.name(), command_id)),
        }
        return Ok(response);
    }
    if object == AppletObject::WindowController {
        let mut response = ServiceResponse::success();
        match request.command_id {
            1 => response.raw_payload = state.resource_user_id.to_le_bytes().to_vec(),
            // These are documented AM no-ops, not host window focus operations.
            10..=12 => {}
            20 => {
                state.window_visible = match request.raw_payload.first() {
                    Some(0) => false,
                    Some(1) => true,
                    _ => {
                        return Err(HorizonError::IpcProtocol(
                            "window visibility requires a boolean".into(),
                        ));
                    }
                }
            }
            21 => {
                let bytes = request.raw_payload.get(..8).ok_or_else(|| {
                    HorizonError::IpcProtocol("GPU time slice requires an s64".into())
                })?;
                let slice = i64::from_le_bytes(bytes.try_into().unwrap());
                if slice < 0 {
                    return Err(HorizonError::IpcProtocol(
                        "GPU time slice must not be negative".into(),
                    ));
                }
                state.gpu_time_slice = slice;
            }
            command_id => return Err(unknown(object.name(), command_id)),
        }
        return Ok(response);
    }
    if object != AppletObject::ApplicationProxy {
        return Err(unknown(object.name(), request.command_id));
    }
    let child = match request.command_id {
        0 => AppletObject::CommonStateGetter,
        1 => AppletObject::SelfController,
        2 => AppletObject::WindowController,
        3 => AppletObject::AudioController,
        4 => AppletObject::DisplayController,
        10 => AppletObject::ProcessWindingController,
        11 => AppletObject::LibraryAppletCreator,
        20 => AppletObject::ApplicationFunctions,
        1000 => AppletObject::DebugFunctions,
        command_id => return Err(unknown(object.name(), command_id)),
    };
    object_response(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termination_result_is_retained_without_overwriting_ipc_success() {
        let mut state = ApplicationState::new(1);
        let other = ApplicationState::new(2);
        let mut request = ServiceRequest {
            service: "IApplicationFunctions".into(),
            command_id: 22,
            values: vec![],
            raw_payload: vec![],
            buffers: vec![],
        };
        for result in [0u32, 0xdeadbeef, u32::MAX] {
            request.raw_payload = result.to_le_bytes().to_vec();
            let response =
                dispatch(AppletObject::ApplicationFunctions, &mut state, &request).unwrap();
            assert_eq!(state.terminate_result, result);
            assert_eq!(response.result_code, 0);
            assert!(response.raw_payload.is_empty());
            assert!(response.objects.is_empty());
            for length in 0..4 {
                request.raw_payload.truncate(length);
                assert!(
                    dispatch(AppletObject::ApplicationFunctions, &mut state, &request).is_err()
                );
                assert_eq!(state.terminate_result, result);
            }
        }
        assert_eq!(other.terminate_result, 0);
    }

    #[test]
    fn self_controller_validates_whole_policy_before_committing() {
        let mut state = ApplicationState::new(1);
        let mut request = ServiceRequest {
            service: "ISelfController".into(),
            command_id: 13,
            raw_payload: vec![1, 0, 1],
            values: vec![],
            buffers: vec![],
        };
        dispatch(AppletObject::SelfController, &mut state, &request).unwrap();
        assert_eq!(state.focus_handling, [true, false, true]);
        for malformed in [vec![], vec![0, 1], vec![0, 1, 2]] {
            request.raw_payload = malformed;
            assert!(dispatch(AppletObject::SelfController, &mut state, &request).is_err());
            assert_eq!(state.focus_handling, [true, false, true]);
        }
        for command in [11, 12, 14, 16] {
            request.command_id = command;
            request.raw_payload = vec![1];
            dispatch(AppletObject::SelfController, &mut state, &request).unwrap();
            request.raw_payload = vec![2];
            assert!(dispatch(AppletObject::SelfController, &mut state, &request).is_err());
        }
        assert!(state.operation_mode_notifications && state.performance_mode_notifications);
        assert!(state.restart_messages && state.out_of_focus_suspending);
        request.command_id = 10;
        request.raw_payload = 2u32.to_le_bytes().to_vec();
        dispatch(AppletObject::SelfController, &mut state, &request).unwrap();
        request.raw_payload = u32::MAX.to_le_bytes().to_vec();
        assert!(dispatch(AppletObject::SelfController, &mut state, &request).is_err());
        assert_eq!(state.screenshot_permission, 2);
        request.command_id = 999;
        assert!(dispatch(AppletObject::SelfController, &mut state, &request).is_err());
    }

    #[test]
    fn message_event_is_level_triggered_and_empty_queue_returns_guest_error() {
        let mut state = ApplicationState::new(9);
        let mut request = ServiceRequest {
            service: "ICommonStateGetter".into(),
            command_id: 0,
            raw_payload: vec![],
            values: vec![],
            buffers: vec![],
        };
        assert_eq!(
            dispatch(AppletObject::CommonStateGetter, &mut state, &request)
                .unwrap()
                .objects,
            vec![ReturnedObject::AppletMessageEvent]
        );
        assert!(state.notification_event.poll());
        assert!(state.notification_event.poll());
        request.command_id = 9;
        assert_eq!(
            dispatch(AppletObject::CommonStateGetter, &mut state, &request)
                .unwrap()
                .raw_payload,
            vec![1]
        );
        assert!(state.notification_event.signaled);
        request.command_id = 1;
        assert_eq!(
            dispatch(AppletObject::CommonStateGetter, &mut state, &request)
                .unwrap()
                .raw_payload,
            15u32.to_le_bytes()
        );
        assert!(!state.notification_event.poll());
        assert_eq!(
            dispatch(AppletObject::CommonStateGetter, &mut state, &request)
                .unwrap()
                .result_code,
            0x680
        );
    }

    #[test]
    fn application_proxy_exposes_typed_children_without_accepting_unknown_methods() {
        let mut request = ServiceRequest {
            service: "appletOE".into(),
            command_id: 0,
            values: vec![],
            raw_payload: vec![0; 8],
            buffers: vec![],
        };
        let profile = HorizonProfileId("22.1.0".into());
        let mut state = ApplicationState::new(1);
        assert_eq!(
            AppletService.call(&profile, &request).unwrap().objects,
            vec![ReturnedObject::Applet(AppletObject::ApplicationProxy)]
        );
        request.raw_payload.clear();
        assert!(AppletService.call(&profile, &request).is_err());
        request.command_id = 20;
        assert_eq!(
            dispatch(AppletObject::ApplicationProxy, &mut state, &request)
                .unwrap()
                .objects,
            vec![ReturnedObject::Applet(AppletObject::ApplicationFunctions)]
        );
        assert!(dispatch(AppletObject::ApplicationFunctions, &mut state, &request).is_err());
        request.command_id = 999;
        assert!(dispatch(AppletObject::ApplicationProxy, &mut state, &request).is_err());
    }

    #[test]
    fn window_identity_and_policy_are_shared_by_controllers_not_between_processes() {
        let mut first = ApplicationState::new(7);
        let mut second = ApplicationState::new(8);
        let mut request = ServiceRequest {
            service: "IWindowController".into(),
            command_id: 1,
            raw_payload: vec![],
            values: vec![],
            buffers: vec![],
        };
        assert_eq!(
            dispatch(AppletObject::WindowController, &mut first, &request)
                .unwrap()
                .raw_payload,
            7u64.to_le_bytes()
        );
        assert_eq!(
            dispatch(AppletObject::WindowController, &mut second, &request)
                .unwrap()
                .raw_payload,
            8u64.to_le_bytes()
        );
        request.command_id = 20;
        request.raw_payload = vec![0];
        dispatch(AppletObject::WindowController, &mut first, &request).unwrap();
        assert!(!first.window_visible);
        assert!(second.window_visible);
        request.raw_payload = vec![2];
        assert!(dispatch(AppletObject::WindowController, &mut first, &request).is_err());
        assert!(!first.window_visible);
        request.command_id = 21;
        request.raw_payload = 123i64.to_le_bytes().to_vec();
        dispatch(AppletObject::WindowController, &mut first, &request).unwrap();
        request.raw_payload = (-1i64).to_le_bytes().to_vec();
        assert!(dispatch(AppletObject::WindowController, &mut first, &request).is_err());
        assert_eq!(first.gpu_time_slice, 123);
    }
}
