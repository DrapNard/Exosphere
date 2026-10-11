// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Minimal Horizon display/layer model independent from Vulkan/Metal.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_millihz: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerScalingMode {
    None,
    FitToLayer,
    PreserveAspectRatio,
}

impl TryFrom<u32> for LayerScalingMode {
    type Error = crate::HorizonError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            2 => Ok(Self::FitToLayer),
            4 => Ok(Self::PreserveAspectRatio),
            _ => Err(crate::HorizonError::IpcProtocol(
                "unsupported VI scaling mode".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerInfo {
    pub id: u64,
    pub display: DisplayId,
    pub width: u32,
    pub height: u32,
    /// Explicit VI policy; None means no policy has been requested yet, not an
    /// inferred display transform. Presentation must consume this retained state.
    pub scaling_mode: Option<LayerScalingMode>,
}

#[derive(Debug)]
pub struct DisplayManager {
    displays: BTreeMap<DisplayId, DisplayInfo>,
    layers: BTreeMap<u64, LayerInfo>,
    managed_owners: BTreeMap<u64, u64>,
    next_layer: u64,
    pub producers: crate::graphic_producer::ProducerRegistry,
}

impl Default for DisplayManager {
    fn default() -> Self {
        let id = DisplayId(1);
        let mut displays = BTreeMap::new();
        displays.insert(
            id,
            DisplayInfo {
                id,
                name: "Default".into(),
                width: 1280,
                height: 720,
                refresh_millihz: 60_000,
                enabled: true,
            },
        );
        Self {
            displays,
            layers: BTreeMap::new(),
            managed_owners: BTreeMap::new(),
            next_layer: 1,
            producers: Default::default(),
        }
    }
}

impl DisplayManager {
    pub fn displays(&self) -> impl Iterator<Item = &DisplayInfo> {
        self.displays.values()
    }
    pub fn display(&self, id: DisplayId) -> Option<&DisplayInfo> {
        self.displays.get(&id)
    }

    pub fn open_default_display(&self) -> Option<DisplayId> {
        self.displays.keys().next().copied()
    }

    pub fn create_layer(
        &mut self,
        display: DisplayId,
        width: u32,
        height: u32,
    ) -> Option<LayerInfo> {
        self.displays.get(&display)?;
        let id = self.next_layer;
        self.next_layer = self.next_layer.wrapping_add(1).max(1);
        let layer = LayerInfo {
            id,
            display,
            width,
            height,
            scaling_mode: None,
        };
        self.layers.insert(id, layer);
        Some(layer)
    }

    pub fn destroy_layer(&mut self, id: u64) -> bool {
        self.producers.release_layer(id);
        self.managed_owners.remove(&id);
        self.layers.remove(&id).is_some()
    }

    pub fn managed_layer(&self, id: u64, resource_id: u64) -> Option<&LayerInfo> {
        if self.managed_owners.get(&id) != Some(&resource_id) {
            return None;
        }
        self.layers.get(&id)
    }

    pub fn set_managed_layer_scaling(
        &mut self,
        id: u64,
        owner: u64,
        mode: LayerScalingMode,
    ) -> crate::HorizonResult<()> {
        if self.managed_owners.get(&id) != Some(&owner) {
            return Err(crate::HorizonError::PermissionDenied);
        }
        let layer = self.layers.get_mut(&id).ok_or_else(|| {
            crate::HorizonError::InvalidState("managed layer does not exist".into())
        })?;
        layer.scaling_mode = Some(mode);
        Ok(())
    }

    pub fn open_managed_native_window(
        &mut self,
        layer: u64,
        owner: u64,
    ) -> crate::HorizonResult<Vec<u8>> {
        self.managed_layer(layer, owner).ok_or_else(|| {
            crate::HorizonError::InvalidState("unknown or foreign managed display layer".into())
        })?;
        let producer = self.producers.create(layer, owner)?;
        self.producers.native_window(producer, owner)
    }

    /// Application-manager allocation, retained until its owning application exits.
    /// Buffer queues and native windows are created separately when VI opens the layer.
    pub fn create_managed_layer(&mut self, resource_id: u64) -> crate::HorizonResult<LayerInfo> {
        if resource_id == 0 {
            return Err(crate::HorizonError::InvalidState(
                "managed layer needs an application identity".into(),
            ));
        }
        let display = self
            .displays
            .values()
            .find(|display| display.name == "Default" && display.enabled)
            .ok_or_else(|| {
                crate::HorizonError::InvalidState("default display is unavailable".into())
            })?;
        let (display_id, width, height) = (display.id, display.width, display.height);
        if self.layers.contains_key(&self.next_layer) || self.next_layer == u64::MAX {
            return Err(crate::HorizonError::InvalidState(
                "display layer identity exhausted".into(),
            ));
        }
        let layer = self
            .create_layer(display_id, width, height)
            .ok_or_else(|| {
                crate::HorizonError::InvalidState("managed display layer allocation failed".into())
            })?;
        self.managed_owners.insert(layer.id, resource_id);
        Ok(layer)
    }

    pub fn release_managed_layers(&mut self, resource_id: u64) {
        self.producers.release_owner(resource_id);
        self.layers
            .retain(|id, _| self.managed_owners.get(id) != Some(&resource_id));
        self.managed_owners.retain(|_, owner| *owner != resource_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_layers_have_distinct_identity_owned_lifetime_and_display_metadata() {
        let mut manager = DisplayManager::default();
        assert!(manager.create_managed_layer(0).is_err());
        let first = manager.create_managed_layer(1).unwrap();
        let second = manager.create_managed_layer(1).unwrap();
        let foreign = manager.create_managed_layer(2).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!((first.width, first.height), (1280, 720));
        assert_eq!(manager.managed_layer(first.id, 1), Some(&first));
        assert!(manager.managed_layer(first.id, 2).is_none());
        manager.release_managed_layers(1);
        assert!(manager.managed_layer(first.id, 1).is_none());
        assert!(manager.managed_layer(second.id, 1).is_none());
        assert_eq!(manager.managed_layer(foreign.id, 2), Some(&foreign));
    }
}
