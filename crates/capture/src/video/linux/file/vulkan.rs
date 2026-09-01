use std::{
    ffi::c_void,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    ptr,
};

use arrayvec::ArrayVec;
use ash::vk::{self, Format, SharingMode};
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use gpui::DMABufferPlane;

#[derive(Debug)]
struct DMAFrame {
    fd: OwnedFd,

    upload_fence: vk::Fence,
    image: vk::Image,

    /// A buffer which is used to copy image to the GPU
    buffer: vk::Buffer,

    // Memory accessible from CPU
    buffer_mapped_memory: *mut c_void,
    command_buffer: vk::CommandBuffer,
}

/// A managed pool of Vulkan textures backed by an exportable DMA-BUF.
/// The main purpose is to simulate screencapturing via Pipewire.
///
/// On Pipewire it works in similar way. It cycles through a pool of
/// pre-allocated DMA-BUFs
pub struct VkDmaBufferPool<const POOL_SIZE: usize> {
    _entry: ash::Entry,
    instance: ash::Instance,

    device: ash::Device,
    physical_device: vk::PhysicalDevice,

    queue: vk::Queue,
    queue_family_index: usize,

    width: u32,
    height: u32,

    vk_format: vk::Format,

    drm_fourcc: DrmFourcc,
    drm_modifier: DrmModifier,

    planes: ArrayVec<DMABufferPlane, 4>,

    frame_pool: ArrayVec<DMAFrame, POOL_SIZE>,
    frame_idx: usize,
}

impl<const POOL_SIZE: usize> Drop for VkDmaBufferPool<POOL_SIZE> {
    fn drop(&mut self) {}
}

pub struct DmaBufferPoolOptions {
    pub width: u32,
    pub height: u32,

    pub vk_format: vk::Format,
}

impl<const POOL_SIZE: usize> VkDmaBufferPool<POOL_SIZE> {
    pub fn new(options: DmaBufferPoolOptions) -> Self {
        if POOL_SIZE == 0 {
            panic!("Invalide pool size");
        }

        unsafe {
            let entry = ash::Entry::linked();

            let application_info = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
            let create_info = vk::InstanceCreateInfo::default().application_info(&application_info);

            let instance = entry
                .create_instance(&create_info, None)
                .expect("Instance creation error");

            let required_device_extensions = [
                ash::khr::external_memory_fd::NAME.as_ptr(),
                ash::ext::external_memory_dma_buf::NAME.as_ptr(),
                ash::ext::image_drm_format_modifier::NAME.as_ptr(),
            ];

            let (physical_device, queue_family_index) = instance
                .enumerate_physical_devices()
                .expect("Failed to get physical devices")
                .into_iter()
                .filter_map(|device| {
                    let supported_extensions = instance
                        .enumerate_device_extension_properties(device)
                        .unwrap();

                    let supports_extensions = required_device_extensions.iter().all(|required| {
                        let required = std::ffi::CStr::from_ptr(*required);

                        supported_extensions.iter().any(|supported| {
                            supported
                                .extension_name_as_c_str()
                                .is_ok_and(|name| name == required)
                        })
                    });

                    supports_extensions
                        .then(|| {
                            instance
                                .get_physical_device_queue_family_properties(device)
                                .iter()
                                .enumerate()
                                .find_map(|(index, info)| {
                                    info.queue_flags
                                        .contains(
                                            vk::QueueFlags::GRAPHICS | vk::QueueFlags::TRANSFER,
                                        )
                                        .then_some((device, index))
                                })
                        })
                        .flatten()
                })
                .max_by_key(|&(device, _)| {
                    let properties = instance.get_physical_device_properties(device);

                    match properties.device_type {
                        vk::PhysicalDeviceType::DISCRETE_GPU => 4,
                        vk::PhysicalDeviceType::INTEGRATED_GPU => 3,
                        vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
                        vk::PhysicalDeviceType::CPU => 1,
                        _ => 0,
                    }
                })
                .expect("Coudn't find a supported GPU");

            let priorities = [1.0];
            let queue_create_info = vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family_index as u32)
                .queue_priorities(&priorities);

            let device_create_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(std::slice::from_ref(&queue_create_info))
                .enabled_extension_names(&required_device_extensions);

            let device = instance
                .create_device(physical_device, &device_create_info, None)
                .expect("Failed to create VkDevice");

            let queue = device.get_device_queue(queue_family_index as u32, 0);

            let drm_fourcc = match options.vk_format {
                Format::R8G8B8A8_UNORM => DrmFourcc::Xbgr8888,
                _ => panic!("Unsupported VkFormat"),
            };

            let mut instance = Self {
                _entry: entry,
                instance,
                physical_device,
                device,
                queue,
                queue_family_index,
                width: options.width,
                height: options.height,
                vk_format: options.vk_format,
                drm_fourcc,
                drm_modifier: DrmModifier::Unrecognized(0),
                frame_idx: 0,
                planes: ArrayVec::new(),
                frame_pool: ArrayVec::new(),
            };

            instance.init_pool();
            instance
        }
    }

    #[hotpath::measure]
    pub fn push_image(&mut self, image: &[u8]) -> gpui::DMABuffer {
        let frame = &mut self.frame_pool[self.frame_idx];

        unsafe {
            ptr::copy_nonoverlapping(
                image.as_ptr(),
                frame.buffer_mapped_memory.cast(),
                image.len(),
            )
        };

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);

        unsafe {
            self.device
                .begin_command_buffer(frame.command_buffer, &begin_info)
                .unwrap();
        }

        // First we make it writable
        let to_transfer = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(frame.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );

        unsafe {
            self.device.cmd_pipeline_barrier(
                frame.command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
        }

        // We can submit as is, Vulkan will handle the destination's DRM modifier.
        // The whole purpose of dealing with Vulkan in the first place
        let copy_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: self.width,
                height: self.height,
                depth: 1,
            });

        unsafe {
            self.device.cmd_copy_buffer_to_image(
                frame.command_buffer,
                frame.buffer,
                frame.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy_region],
            );
        }

        // Now we want to make that image accessible to consumers
        let to_general = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(frame.image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );

        unsafe {
            self.device.cmd_pipeline_barrier(
                frame.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_general],
            );

            self.device
                .end_command_buffer(frame.command_buffer)
                .expect("Failed to submit GPU work")
        };

        let command_buffers = [frame.command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);

        unsafe {
            self.device
                .queue_submit(self.queue, &[submit_info], frame.upload_fence)
                .unwrap();

            // I faced a bug where handoff to the encoder was too fast
            // and the frame was only partially encoded.
            // This line forces the CPU to wait GPU till the work completion
            // Vulkan is hard...
            self.device
                .wait_for_fences(&[frame.upload_fence], true, u64::MAX)
                .expect("Failed to wait for Vulkan upload");
        };

        self.frame_idx = (self.frame_idx + 1) % POOL_SIZE;
        gpui::DMABuffer::new(
            frame.fd.as_raw_fd(),
            self.width,
            self.height,
            DrmFormat {
                code: self.drm_fourcc,
                modifier: self.drm_modifier,
            },
            &self.planes,
        )
    }

    fn init_pool(&mut self) {
        let modifier = self
            .enumerate_drm_modifiers()
            .into_iter()
            .find(|modifier| self.modifier_is_exportable(modifier.drm_format_modifier))
            .expect("Can't find an exportable modifier");

        self.drm_modifier = DrmModifier::from(modifier.drm_format_modifier);

        for _ in 0..POOL_SIZE {
            // From: https://registry.khronos.org/VulkanSC/specs/1.0-extensions/man/html/VK_EXT_image_drm_format_modifier.html
            let selected_modifiers = [modifier.drm_format_modifier];
            let mut modifier_create_info = vk::ImageDrmFormatModifierListCreateInfoEXT::default()
                .drm_format_modifiers(&selected_modifiers);

            let mut external_image_info = vk::ExternalMemoryImageCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            let image_create_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(self.vk_format)
                .extent(vk::Extent3D {
                    width: self.width,
                    height: self.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .push_next(&mut modifier_create_info)
                .push_next(&mut external_image_info);

            let image = unsafe { self.device.create_image(&image_create_info, None) }
                .expect("Failed to create vkImage");

            let requirements = unsafe { self.device.get_image_memory_requirements(image) };
            let memory_properties = unsafe {
                self.instance
                    .get_physical_device_memory_properties(self.physical_device)
            };

            let memory_type_index = (0..memory_properties.memory_type_count)
                .find(|&index| {
                    let supported = (requirements.memory_type_bits & (1_u32 << index)) != 0;
                    let flags = memory_properties.memory_types[index as usize].property_flags;

                    supported && flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .expect("can't find a compatible memory type");

            let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut export_info = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type_index)
                .push_next(&mut dedicated_info)
                .push_next(&mut export_info);

            let image_memory = unsafe {
                self.device
                    .allocate_memory(&allocate_info, None)
                    .expect("failed to allocate memory")
            };

            unsafe {
                self.device
                    .bind_image_memory(image, image_memory, 0)
                    .expect("Failed to bind memory to image")
            };

            let external_memory_fd_loader =
                ash::khr::external_memory_fd::Device::new(&self.instance, &self.device);

            let fd_info = vk::MemoryGetFdInfoKHR::default()
                .memory(image_memory)
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            let fd = unsafe {
                OwnedFd::from_raw_fd(
                    external_memory_fd_loader
                        .get_memory_fd(&fd_info)
                        .expect("Unable to get DMA-BUF descriptor"),
                )
            };

            // At this point we have DMA-BUF, now we need create a staging buffer
            // through which we can upload actual pixels

            let buffer_size = match self.vk_format {
                Format::R8G8B8A8_UNORM | Format::B8G8R8A8_UNORM => self.width * self.height * 4,
                _ => todo!("unsupported"),
            } as vk::DeviceSize;

            let buffer_info = vk::BufferCreateInfo::default()
                .size(buffer_size)
                .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);

            let buffer = unsafe {
                self.device
                    .create_buffer(&buffer_info, None)
                    .expect("Failed to create stagin buffer")
            };

            let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };

            let memory_type_index = (0..memory_properties.memory_type_count)
                .find(|&index| {
                    let supported = (requirements.memory_type_bits & (1_u32 << index)) != 0;
                    let flags = memory_properties.memory_types[index as usize].property_flags;

                    supported
                        && flags.contains(
                            vk::MemoryPropertyFlags::HOST_VISIBLE
                                | vk::MemoryPropertyFlags::HOST_COHERENT,
                        )
                })
                .expect("can't find a compatible memory type");

            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type_index);

            let buffer_memory = unsafe {
                self.device
                    .allocate_memory(&allocate_info, None)
                    .expect("failed to allocate memory")
            };

            unsafe {
                self.device
                    .bind_buffer_memory(buffer, buffer_memory, 0)
                    .expect("Failed to bind memory for staging buffer");
            }

            let buffer_mapped_memory = unsafe {
                self.device
                    .map_memory(buffer_memory, 0, buffer_size, vk::MemoryMapFlags::empty())
                    .expect("Failed to map memory")
            };

            let cmd_pool_create_info = vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                .queue_family_index(self.queue_family_index as u32);
            let command_pool = unsafe {
                self.device
                    .create_command_pool(&cmd_pool_create_info, None)
                    .expect("Failed to create VkCommandPool")
            };

            let command_buffer_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);

            let command_buffer = unsafe {
                self.device
                    .allocate_command_buffers(&command_buffer_info)
                    .expect("Failed to allocate VkCommandBuffer")
            }[0];

            let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
            let fence = unsafe {
                self.device
                    .create_fence(&fence_info, None)
                    .expect("Failed to create upload fence")
            };

            self.frame_pool.push(DMAFrame {
                fd,
                image,
                buffer,
                upload_fence: fence,
                buffer_mapped_memory,
                command_buffer,
            });
        }

        // Get DRM modifier planes
        for plane in 0..modifier.drm_format_modifier_plane_count {
            let aspect_mask = match plane {
                0 => vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
                1 => vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
                2 => vk::ImageAspectFlags::MEMORY_PLANE_2_EXT,
                3 => vk::ImageAspectFlags::MEMORY_PLANE_3_EXT,
                _ => unreachable!(),
            };

            let subresource = vk::ImageSubresource {
                aspect_mask,
                mip_level: 0,
                array_layer: 0,
            };

            let layout = unsafe {
                self.device
                    .get_image_subresource_layout(self.frame_pool[0].image, subresource)
            };
            self.planes.push(DMABufferPlane {
                offset: layout.offset as usize,
                stride: layout.row_pitch as usize,
            });
        }
    }

    fn modifier_is_exportable(&self, modifier: u64) -> bool {
        let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

        let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(modifier)
            .sharing_mode(SharingMode::EXCLUSIVE);

        let image_format_info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(self.vk_format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::empty())
            .push_next(&mut external_info)
            .push_next(&mut modifier_info);

        let mut external_properties = vk::ExternalImageFormatProperties::default();
        let mut image_properties =
            vk::ImageFormatProperties2::default().push_next(&mut external_properties);

        unsafe {
            if self
                .instance
                .get_physical_device_image_format_properties2(
                    self.physical_device,
                    &image_format_info,
                    &mut image_properties,
                )
                .is_err()
            {
                return false;
            }
        }

        external_properties
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE)
    }

    fn enumerate_drm_modifiers(&self) -> Vec<vk::DrmFormatModifierPropertiesEXT> {
        unsafe {
            let mut modifiers_list = vk::DrmFormatModifierPropertiesListEXT::default();
            let mut format_properties =
                vk::FormatProperties2::default().push_next(&mut modifiers_list);

            self.instance.get_physical_device_format_properties2(
                self.physical_device,
                self.vk_format,
                &mut format_properties,
            );

            let modifiers_count = modifiers_list.drm_format_modifier_count as usize;
            if modifiers_count == 0 {
                return vec![];
            }

            let mut modifiers =
                vec![vk::DrmFormatModifierPropertiesEXT::default(); modifiers_count];

            let mut modifiers_list = vk::DrmFormatModifierPropertiesListEXT::default()
                .drm_format_modifier_properties(&mut modifiers);
            let mut format_properties =
                vk::FormatProperties2::default().push_next(&mut modifiers_list);

            self.instance.get_physical_device_format_properties2(
                self.physical_device,
                self.vk_format,
                &mut format_properties,
            );

            let written = modifiers_list.drm_format_modifier_count as usize;
            modifiers.truncate(written.min(modifiers_count));
            modifiers
        }
    }
}
