//! Optimized regression qualification for the GTK3-compatible GLib backport.
#[cfg(test)]
mod tests {
    use glib::variant::ToVariant;

    #[test]
    fn variant_str_iterator_output_pointer_is_valid_under_optimization() {
        let values = ["zero", "one", "two", "three", "four", "five"];
        let variant = values.to_variant();
        assert_eq!(
            variant.array_iter_str().unwrap().collect::<Vec<_>>(),
            values
        );
        let mut iter = variant.array_iter_str().unwrap();
        assert_eq!(iter.next(), Some("zero"));
        assert_eq!(iter.next_back(), Some("five"));
        assert_eq!(iter.nth(1), Some("two"));
        assert_eq!(iter.nth_back(1), Some("three"));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
        assert_eq!(variant.array_iter_str().unwrap().last(), Some("five"));
        assert_eq!(variant.array_iter_str().unwrap().nth(usize::MAX), None);
        assert_eq!(variant.array_iter_str().unwrap().nth_back(usize::MAX), None);
        let empty = Vec::<String>::new().to_variant();
        assert_eq!(empty.array_iter_str().unwrap().next(), None);
        assert_eq!(empty.array_iter_str().unwrap().next_back(), None);
        assert_eq!(empty.array_iter_str().unwrap().last(), None);
    }

    #[test]
    fn empty_ptr_slice_to_slice_stays_usable_and_releases_objects() {
        use glib::prelude::*;
        use glib::{Object, PtrSlice, Slice};
        for source in [PtrSlice::<Object>::new(), PtrSlice::with_capacity(1), {
            let mut source = PtrSlice::with_capacity(1);
            source.push(Object::new::<Object>());
            source.clear();
            source
        }] {
            let mut destination = Slice::from(source);
            assert!(destination.is_empty());
            let object = Object::new::<Object>();
            let weak = object.downgrade();
            destination.push(object);
            assert!(weak.upgrade().is_some());
            drop(destination);
            assert!(weak.upgrade().is_none());
        }
    }

    #[test]
    fn empty_slice_to_ptr_slice_stays_usable_and_releases_objects() {
        use glib::prelude::*;
        use glib::{IntoPtrSlice, Object, PtrSlice, Slice};
        for source in [Slice::<Object>::new(), Slice::with_capacity(1), {
            let mut source = Slice::with_capacity(1);
            source.push(Object::new::<Object>());
            source.clear();
            source
        }] {
            let mut destination = PtrSlice::from(source);
            assert!(destination.is_empty());
            (&destination).run_with_ptr_slice(|pointers| {
                assert_eq!(pointers.len(), 1);
                assert!(pointers[0].is_null());
            });
            let object = Object::new::<Object>();
            let weak = object.downgrade();
            destination.push(object);
            assert!(weak.upgrade().is_some());
            drop(destination);
            assert!(weak.upgrade().is_none());
        }
    }

    #[test]
    fn nonempty_collection_round_trip_transfers_object_ownership() {
        use glib::prelude::*;
        use glib::{IntoPtrSlice, Object, PtrSlice, Slice};
        for capacity in [1, 4] {
            let object = Object::new::<Object>();
            let weak = object.downgrade();
            let mut source = Slice::with_capacity(capacity);
            source.push(object);
            let pointer = source.as_ptr();
            let pointers = PtrSlice::from(source);
            assert_eq!(pointers.len(), 1);
            (&pointers).run_with_ptr_slice(|array| {
                assert_eq!(array.len(), 2);
                assert!(!array[0].is_null());
                assert!(array[1].is_null());
            });
            assert!(weak.upgrade().is_some());
            let slice = Slice::from(pointers);
            assert_eq!(slice.len(), 1);
            if capacity > 1 {
                assert_eq!(slice.as_ptr(), pointer);
            }
            assert_eq!(slice[0], weak.upgrade().unwrap());
            drop(slice);
            assert!(weak.upgrade().is_none());
        }
    }

    #[test]
    fn empty_date_container_conversion_returns_null() {
        use glib::translate::ToGlibPtr;
        use glib::{Date, DateMonth, Slice};
        for source in [Slice::<Date>::new(), Slice::with_capacity(1), {
            let mut source = Slice::with_capacity(1);
            source.push(Date::from_dmy(1, DateMonth::January, 2026).unwrap());
            source.clear();
            source
        }] {
            let container: *mut glib::ffi::GDate = source.to_glib_container().0;
            assert!(container.is_null());
        }
    }
}

#[cfg(all(test, unix))]
mod spawn_tests {
    use std::{fs::File, io::Read, path::Path};

    #[test]
    fn failed_pipe_spawn_returns_error_and_success_transfers_valid_files() {
        let failed: Result<(glib::Pid, File, File, File), glib::Error> =
            glib::spawn_async_with_pipes(
                "/",
                &[Path::new("/labby-glib-regression-nonexistent-executable")],
                &[],
                glib::SpawnFlags::empty(),
                None,
            );
        assert!(failed.is_err());
        let (_, stdin, mut stdout, mut stderr): (glib::Pid, File, File, File) =
            glib::spawn_async_with_pipes(
                "/",
                &[
                    Path::new("/bin/sh"),
                    Path::new("-c"),
                    Path::new("printf glib-spawn-ok"),
                ],
                &[],
                glib::SpawnFlags::empty(),
                None,
            )
            .unwrap();
        drop(stdin);
        let mut output = String::new();
        stdout.read_to_string(&mut output).unwrap();
        assert_eq!(output, "glib-spawn-ok");
        let mut errors = String::new();
        stderr.read_to_string(&mut errors).unwrap();
        assert!(errors.is_empty());
    }

    #[test]
    fn failed_fd_spawn_returns_error_and_valid_spawn_succeeds() {
        let input = File::open("/dev/null").unwrap();
        let output = File::options().write(true).open("/dev/null").unwrap();
        let failed = glib::spawn_async_with_fds(
            "/",
            &["/labby-glib-regression-nonexistent-executable"],
            &[],
            glib::SpawnFlags::empty(),
            None,
            input.try_clone().unwrap(),
            output.try_clone().unwrap(),
            output.try_clone().unwrap(),
        );
        assert!(failed.is_err());
        let pid = glib::spawn_async_with_fds(
            "/",
            &["/bin/true"],
            &[],
            glib::SpawnFlags::empty(),
            None,
            input.try_clone().unwrap(),
            output.try_clone().unwrap(),
            output.try_clone().unwrap(),
        )
        .unwrap();
        assert_ne!(pid.0, 0);
    }
}

#[cfg(test)]
mod string_builder_tests {
    #[test]
    fn cloning_nonempty_builder_preserves_text_and_independent_append() {
        let original = glib::GStringBuilder::new("abc");
        let mut copy = original.clone();
        assert_eq!(copy.as_str(), "abc");
        copy.append("d");
        assert_eq!(copy.as_str(), "abcd");
        assert_eq!(original.as_str(), "abc");
    }
}

#[cfg(test)]
mod date_tests {
    #[test]
    fn parsing_valid_date_commits_parsed_value() {
        let mut date = glib::Date::from_dmy(1, glib::DateMonth::January, 2020).unwrap();
        date.set_parse("2026-10-24").unwrap();
        assert_eq!(
            (date.year(), date.month(), date.day()),
            (2026, glib::DateMonth::October, 24)
        );
    }

    #[test]
    fn parsing_invalid_date_preserves_original() {
        let mut date = glib::Date::from_dmy(1, glib::DateMonth::January, 2020).unwrap();
        let original = date;
        assert!(date.set_parse("definitely-not-a-date").is_err());
        assert_eq!(date, original);
    }

    #[test]
    fn subtracting_days_accepts_valid_reduction_and_zero() {
        let mut date = glib::Date::from_dmy(2, glib::DateMonth::February, 2026).unwrap();
        date.subtract_days(1).unwrap();
        assert_eq!(date.day(), 1);
        let original = date;
        date.subtract_days(0).unwrap();
        assert_eq!(date, original);
    }

    #[test]
    fn subtracting_days_rejects_zero_julian_and_underflow_without_mutation() {
        let mut date = glib::Date::from_julian(1).unwrap();
        let original = date;
        assert!(date.subtract_days(1).is_err());
        assert_eq!(date, original);
        assert!(date.subtract_days(u32::MAX).is_err());
        assert_eq!(date, original);
    }
}

#[cfg(test)]
mod list_tests {
    use glib::prelude::*;

    #[test]
    fn list_retain_sees_actual_items_and_releases_removed_head_nodes() {
        let objects: Vec<glib::Object> = (0..3)
            .map(|_| glib::Object::new::<glib::Object>())
            .collect();
        let pointers: Vec<_> = objects.iter().map(|object| object.as_ptr()).collect();
        let weak: Vec<_> = objects.iter().map(|object| object.downgrade()).collect();
        let mut list: glib::List<glib::Object> = objects.into_iter().collect();
        let mut index = 0;
        list.retain(|object| {
            assert_eq!(object.as_ptr(), pointers[index]);
            index += 1;
            index == 3
        });
        assert_eq!(index, 3);
        assert_eq!(list.len(), 1);
        assert!(weak[0].upgrade().is_none());
        assert!(weak[1].upgrade().is_none());
        assert!(weak[2].upgrade().is_some());
        list.retain(|_| false);
        assert!(list.is_empty());
        assert!(weak[2].upgrade().is_none());
    }

    #[test]
    fn slist_retain_sees_actual_items_and_releases_removed_head_nodes() {
        let objects: Vec<glib::Object> = (0..3)
            .map(|_| glib::Object::new::<glib::Object>())
            .collect();
        let pointers: Vec<_> = objects.iter().map(|object| object.as_ptr()).collect();
        let weak: Vec<_> = objects.iter().map(|object| object.downgrade()).collect();
        let mut list: glib::SList<glib::Object> = objects.into_iter().collect();
        let mut index = 0;
        list.retain(|object| {
            assert_eq!(object.as_ptr(), pointers[index]);
            index += 1;
            index == 3
        });
        assert_eq!(index, 3);
        assert_eq!(list.len(), 1);
        assert!(weak[0].upgrade().is_none());
        assert!(weak[1].upgrade().is_none());
        assert!(weak[2].upgrade().is_some());
        list.retain(|_| false);
        assert!(list.is_empty());
        assert!(weak[2].upgrade().is_none());
    }
}

#[cfg(test)]
mod owned_variant_tests {
    #[test]
    fn owned_variant_data_and_fixed_array_survive_transfer_to_another_thread() {
        use glib::variant::ToVariant;
        let expected = 0x12345678u32;
        let original = expected.to_variant();
        let data = original.data_as_bytes().as_ref().to_vec();
        let value = glib::Variant::from_data::<u32, _>(data);
        let fixed: glib::Variant =
            glib::variant::FixedSizeVariantArray::<_, u32>::from(vec![1, 2, 3]).into();
        drop(original);
        std::thread::spawn(move || {
            assert_eq!(value.get::<u32>(), Some(expected));
            assert_eq!(fixed.fixed_array::<u32>().unwrap(), &[1, 2, 3]);
        })
        .join()
        .unwrap();
    }
}

// Raw FFI here verifies the container ownership contracts at their native boundary.
#[cfg(test)]
#[allow(unsafe_code)]
mod native_container_tests {

    use glib::{prelude::*, translate::ToGlibPtr};

    #[test]
    fn empty_list_container() {
        let original = glib::List::<glib::Object>::new();
        let pointer: *mut glib::ffi::GList = original.to_glib_container().0;
        assert!(pointer.is_null());
    }
    #[test]
    fn empty_slist_container() {
        let original = glib::SList::<glib::Object>::new();
        let pointer: *mut glib::ffi::GSList = original.to_glib_container().0;
        assert!(pointer.is_null());
    }
    #[test]
    fn list_container_ownership() {
        let object = glib::Object::new::<glib::Object>();
        let weak = object.downgrade();
        // SAFETY: this freshly created object is live and unmodified.
        let expected_class = unsafe { (*object.as_ptr()).g_type_instance.g_class };
        let original: glib::List<glib::Object> = [object.clone()].into_iter().collect();
        // SAFETY: g_list_copy receives an existing live list and transfers newly
        // allocated container nodes whose items remain borrowed from original.
        let pointer = unsafe { glib::ffi::g_list_copy(original.as_ptr() as *mut _) };
        // SAFETY: pointer owns those nodes; original keeps borrowed items alive.
        let adopted = unsafe { glib::List::<glib::Object>::from_glib_container(pointer) };
        assert_eq!(adopted.front().unwrap().as_ptr(), object.as_ptr());
        // SAFETY: object remains a live GObject. A constructor must never overwrite
        // its GTypeInstance header while taking references to container elements.
        let actual_class = unsafe { (*object.as_ptr()).g_type_instance.g_class };
        assert_eq!(actual_class, expected_class);
        drop(original);
        drop(object);
        assert!(weak.upgrade().is_some());
        drop(adopted);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn slist_container_ownership() {
        let object = glib::Object::new::<glib::Object>();
        let weak = object.downgrade();
        // SAFETY: this freshly created object is live and unmodified.
        let expected_class = unsafe { (*object.as_ptr()).g_type_instance.g_class };
        let original: glib::SList<glib::Object> = [object.clone()].into_iter().collect();
        // SAFETY: a live list is copied; new nodes borrow their existing items.
        let pointer = unsafe { glib::ffi::g_slist_copy(original.as_ptr() as *mut _) };
        // SAFETY: pointer owns nodes and original keeps the items alive.
        let adopted = unsafe { glib::SList::<glib::Object>::from_glib_container(pointer) };
        assert_eq!(adopted.front().unwrap().as_ptr(), object.as_ptr());
        // SAFETY: object remains live; adoption must preserve its header.
        assert_eq!(
            unsafe { (*object.as_ptr()).g_type_instance.g_class },
            expected_class
        );
        drop(original);
        drop(object);
        assert!(weak.upgrade().is_some());
        drop(adopted);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn nonempty_containers() {
        let objects: Vec<glib::Object> = (0..3)
            .map(|_| glib::Object::new::<glib::Object>())
            .collect();
        let list: glib::List<glib::Object> = objects.iter().cloned().collect();
        let slist: glib::SList<glib::Object> = objects.into_iter().collect();
        let a = list.to_glib_container().0;
        let b = slist.to_glib_container().0;
        // SAFETY: to_glib_container transfers independent list nodes, with borrowed
        // elements kept alive here. Free only nodes, then prove originals survive.
        unsafe {
            assert_eq!(glib::ffi::g_list_length(a), 3);
            assert_eq!(glib::ffi::g_slist_length(b), 3);
            glib::ffi::g_list_free(a);
            glib::ffi::g_slist_free(b);
        }
        assert_eq!(list.len(), 3);
        assert_eq!(slist.len(), 3);
    }
    #[test]
    #[allow(deprecated)]
    fn zero_capacity_try_send_does_not_wait() {
        let (sender, receiver) = glib::MainContext::sync_channel::<u32>(glib::Priority::DEFAULT, 0);
        let (done, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || done.send(sender.try_send(7)).unwrap());
        let immediate = result.recv_timeout(std::time::Duration::from_millis(200));
        // Dropping the real receiver releases a legacy blocked try_send, so the
        // regression never leaves an orphan thread after its bounded observation.
        drop(receiver);
        worker.join().unwrap();
        assert!(matches!(
            immediate,
            Ok(Err(std::sync::mpsc::TrySendError::Full(7)))
        ));
    }
}

/// Borrowed storage must not escape through a lifetime-free Variant.
/// ```compile_fail,E0597
/// let data = vec![0u8; 4];
/// let _value = glib::Variant::from_data::<u32, _>(data.as_slice());
/// ```
/// Non-thread-safe owners must not enter Variant's Send + Sync storage.
/// ```compile_fail,E0277
/// use std::rc::Rc;
/// struct Owner(Rc<Vec<u8>>);
/// impl AsRef<[u8]> for Owner {
///     fn as_ref(&self) -> &[u8] { self.0.as_slice() }
/// }
/// let _value = glib::Variant::from_data::<u32, _>(Owner(Rc::new(vec![0; 4])));
/// ```
/// Borrowed fixed arrays must not escape through the consuming conversion.
/// ```compile_fail,E0597
/// let data = vec![1u32, 2, 3];
/// let _value: glib::Variant =
///     glib::variant::FixedSizeVariantArray::<_, u32>::from(data.as_slice()).into();
/// ```
pub struct VariantOwnerCompileContracts;

#[cfg(test)]
mod uri_segment_tests {
    #[test]
    fn segment_end_is_a_bounded_offset_into_one_input() {
        assert_eq!(
            glib::Uri::unescape_segment(Some("a%20b-tail"), Some(5), None).as_deref(),
            Some("a b")
        );
        assert_eq!(
            glib::Uri::unescape_segment(Some("a%20b"), None, None).as_deref(),
            Some("a b")
        );
        assert_eq!(
            glib::Uri::unescape_segment(Some("a%20b"), Some(0), None).as_deref(),
            Some("")
        );
        assert!(glib::Uri::unescape_segment(Some("abc"), Some(4), None).is_none());
        assert!(glib::Uri::unescape_segment(None, Some(0), None).is_none());
        assert!(glib::Uri::unescape_segment(Some("%00"), None, None).is_none());
        assert!(glib::Uri::unescape_segment(Some("%2f"), None, Some("/")).is_none());
        assert!(glib::Uri::unescape_segment(Some("%20"), Some(2), None).is_none());
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod boxed_array_tests {
    #[test]
    fn full_inline_array_copies_every_date_and_handles_empty() {
        use glib::translate::ToGlibContainerFromSlice;
        let empty = <glib::Date as ToGlibContainerFromSlice<*mut glib::ffi::GDate>>::to_glib_full_from_slice(&[]);
        assert!(empty.is_null());
        let dates: Vec<_> = (1..=31)
            .map(|day| glib::Date::from_dmy(day, glib::DateMonth::January, 2026).unwrap())
            .collect();
        let copied = <glib::Date as ToGlibContainerFromSlice<*mut glib::ffi::GDate>>::to_glib_full_from_slice(&dates);
        // SAFETY: the conversion transfers an array with dates.len() initialized
        // GDate values. Date has no separately owned fields; free the array once.
        unsafe {
            for (index, date) in dates.iter().enumerate() {
                assert_eq!(glib::ffi::g_date_get_day(copied.add(index)), date.day());
                assert_eq!(glib::ffi::g_date_get_year(copied.add(index)), date.year());
            }
            glib::ffi::g_free(copied.cast());
        }
        assert_eq!(dates.last().unwrap().day(), 31);
    }
}

/// Local task outputs must not cross threads through a JoinHandle.
/// ```compile_fail,E0277
/// fn require_send<T: Send>(_: T) {}
/// let context = glib::MainContext::new();
/// let handle = context.spawn_local(async { std::rc::Rc::new(1) });
/// require_send(handle);
/// ```
/// Cross-thread spawning must require a transferable result.
/// ```compile_fail,E0277
/// let context = glib::MainContext::new();
/// let _handle = context.spawn_from_within(|| async { std::rc::Rc::new(1) });
/// ```
pub struct TaskResultCompileContracts;

#[cfg(test)]
mod task_and_translation_tests {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    #[test]
    fn aborted_handle_reports_cancellation_without_waiting_for_source_finalization() {
        let context = glib::MainContext::new();
        let mut handle = context.spawn(std::future::pending::<u32>());
        handle.abort();
        assert!(futures_core::future::FusedFuture::is_terminated(&handle));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(
            matches!(Pin::new(&mut handle).poll(&mut cx), Poll::Ready(Err(error)) if error.is_cancelled())
        );
        assert!(futures_core::future::FusedFuture::is_terminated(&handle));
    }

    #[test]
    fn sendable_task_result_can_still_cross_threads() {
        let context = glib::MainContext::new();
        let handle = context.spawn(async { 42u32 });
        while context.pending() {
            context.iteration(false);
        }
        std::thread::spawn(move || {
            let mut handle = handle;
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            assert!(matches!(
                Pin::new(&mut handle).poll(&mut cx),
                Poll::Ready(Ok(42))
            ));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn translation_accepts_only_offsets_within_utf8_boundaries() {
        let domain = Some("labby-regression-domain-with-no-catalog");
        assert_eq!(glib::dpgettext(domain, "context\u{4}message", 8), "message");
        assert_eq!(glib::dpgettext(domain, "abc", 3), "");
        assert!(std::panic::catch_unwind(|| glib::dpgettext(domain, "abc", 4)).is_err());
        assert!(std::panic::catch_unwind(|| glib::dpgettext(domain, "é", 1)).is_err());
    }
}

/// Log fields must not outlive their borrowed value bytes.
/// ```compile_fail,E0597
/// let field;
/// {
///     let bytes = vec![1u8, 2, 3];
///     field = glib::LogField::new(glib::gstr!("MESSAGE"), &bytes);
/// }
/// let _ = field.value_bytes();
/// ```
pub struct LogFieldCompileContract;

#[cfg(test)]
mod string_vector_and_flags_tests {
    #[test]
    fn string_vector_reserve_clear_and_reuse_preserve_termination() {
        drop(glib::StrV::with_capacity(8));
        let mut values = glib::StrV::from(["one", "two"]);
        values.clear();
        assert!(values.is_empty());
        assert_eq!(values.join(None::<&str>), "");
        values.push(glib::GString::from("three"));
        assert_eq!(values.join(None::<&str>), "three");
        values.clear();
        drop(values);
        let mut empty = glib::StrV::new();
        empty.clear();
        empty.reserve(4);
        drop(empty);
    }

    #[test]
    fn string_vector_equality_requires_matching_lengths() {
        let value = glib::StrV::from(["one"]);
        assert!(value == ["one"][..]);
        assert!(value != ["one", "two"][..]);
        assert!(value != [][..]);
        assert!(glib::StrV::new() != ["one"][..]);
    }

    #[test]
    fn repeated_flag_nicks_are_an_idempotent_union() {
        let class = glib::FlagsClass::new::<glib::BindingFlags>();
        let expected = glib::BindingFlags::BIDIRECTIONAL.bits();
        assert_eq!(
            class
                .from_nick_string("bidirectional|bidirectional")
                .unwrap(),
            expected
        );
        assert_eq!(
            class
                .from_nick_string("bidirectional|sync-create|bidirectional")
                .unwrap(),
            expected | glib::BindingFlags::SYNC_CREATE.bits()
        );
    }

    #[test]
    fn log_field_preserves_live_binary_and_empty_values() {
        let bytes = [0, 1, 2];
        let field = glib::LogField::new(glib::gstr!("MESSAGE"), &bytes);
        assert_eq!(field.value_bytes(), Some(bytes.as_slice()));
        assert_eq!(
            glib::LogField::new(glib::gstr!("EMPTY"), &[]).value_bytes(),
            Some([].as_slice())
        );
    }
}

#[cfg(all(test, unix))]
mod spawn_callback_ownership_tests {
    #[test]
    fn parent_releases_setup_captures_on_spawn_success_and_error() {
        use std::{path::Path, sync::Arc};
        for path in ["/bin/true", "/labby-no-such-glib-spawn-executable"] {
            let capture = Arc::new(());
            let weak = Arc::downgrade(&capture);
            let result = glib::spawn_async(
                None::<&Path>,
                &[Path::new(path)],
                &[],
                glib::SpawnFlags::empty(),
                Some(Box::new(move || drop(capture))),
            );
            assert_eq!(result.is_ok(), path == "/bin/true");
            assert!(weak.upgrade().is_none());
            let capture = Arc::new(());
            let weak = Arc::downgrade(&capture);
            let result: Result<(glib::Pid, std::fs::File, std::fs::File, std::fs::File), _> =
                glib::spawn_async_with_pipes(
                    "/",
                    &[Path::new(path)],
                    &[],
                    glib::SpawnFlags::empty(),
                    Some(Box::new(move || drop(capture))),
                );
            assert_eq!(result.is_ok(), path == "/bin/true");
            assert!(weak.upgrade().is_none());
            let capture = Arc::new(());
            let weak = Arc::downgrade(&capture);
            let null = std::fs::File::options()
                .read(true)
                .write(true)
                .open("/dev/null")
                .unwrap();
            let result = glib::spawn_async_with_fds(
                "/",
                &[path],
                &[],
                glib::SpawnFlags::empty(),
                Some(Box::new(move || drop(capture))),
                null.try_clone().unwrap(),
                null.try_clone().unwrap(),
                null.try_clone().unwrap(),
            );
            assert_eq!(result.is_ok(), path == "/bin/true");
            assert!(weak.upgrade().is_none());
        }
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod gstring_ownership_tests {
    #[test]
    fn owned_string_pointer_transfer_preserves_allocation_until_consumer_free() {
        use glib::translate::IntoGlibPtr;
        let strings = glib::StrV::from(["owned-pointer"]);
        let owned = strings.as_slice()[0].clone();
        // SAFETY: owned is a live GLib string and transfers one owning reference.
        // The consumer reads it and frees it exactly once.
        unsafe {
            let pointer = owned.into_glib_ptr();
            assert_eq!(
                std::ffi::CStr::from_ptr(pointer).to_bytes(),
                b"owned-pointer"
            );
            glib::ffi::g_free(pointer.cast());
        }
        assert_eq!(strings[0].as_str(), "owned-pointer");
    }

    #[test]
    fn foreign_string_bytes_and_cstring_preserve_text_and_terminator() {
        for text in ["", "abc", "é🙂"] {
            let make = || glib::GStringBuilder::new(text).into_string();
            assert_eq!(make().into_bytes(), text.as_bytes());
            let mut nul = text.as_bytes().to_vec();
            nul.push(0);
            assert_eq!(make().into_bytes_with_nul(), nul);
            assert_eq!(
                std::ffi::CString::try_from(make())
                    .unwrap()
                    .to_bytes_with_nul(),
                nul
            );
        }
    }

    #[test]
    fn owned_string_vector_value_roundtrip_preserves_every_storage_form() {
        for strings in [
            vec![],
            vec![
                glib::GString::from("inline"),
                glib::GString::from("x".repeat(100)),
                glib::GStringBuilder::new("foreign").into_string(),
            ],
        ] {
            let expected: Vec<_> = strings.iter().map(|s| s.as_str().to_owned()).collect();
            let value = glib::Value::from(strings);
            assert_eq!(value.get::<Vec<String>>().unwrap(), expected);
            let cloned = value.clone();
            drop(value);
            assert_eq!(cloned.get::<Vec<String>>().unwrap(), expected);
        }
    }
}

#[cfg(test)]
mod retain_matrix_tests {
    use glib::prelude::*;

    #[test]
    fn both_list_types_cover_head_middle_tail_all_and_no_removals() {
        macro_rules! check {
            ($list:ident) => {
                for keep in 0u8..8 {
                    let objects: Vec<glib::Object> = (0..3)
                        .map(|_| glib::Object::new::<glib::Object>())
                        .collect();
                    let pointers: Vec<_> = objects.iter().map(|object| object.as_ptr()).collect();
                    let weak: Vec<_> = objects.iter().map(|object| object.downgrade()).collect();
                    let mut list: glib::$list<glib::Object> = objects.into_iter().collect();
                    let mut index = 0;
                    list.retain(|object| {
                        assert_eq!(object.as_ptr(), pointers[index]);
                        let retain = keep & (1 << index) != 0;
                        index += 1;
                        retain
                    });
                    assert_eq!(index, 3);
                    assert_eq!(list.len(), keep.count_ones() as usize);
                    for (index, object) in weak.iter().enumerate() {
                        assert_eq!(object.upgrade().is_some(), keep & (1 << index) != 0);
                    }
                    drop(list);
                    assert!(weak.iter().all(|object| object.upgrade().is_none()));
                }
            };
        }
        check!(List);
        check!(SList);
    }
}

#[cfg(test)]
mod uri_utf8_tests {
    #[test]
    fn uri_unescape_rejects_invalid_decoded_utf8_and_split_input_codepoints() {
        assert!(glib::Uri::unescape_segment(Some("é"), Some(1), None).is_none());
        for encoded in ["%ff", "%c3", "%ed%a0%80", "%f4%90%80%80"] {
            assert!(glib::Uri::unescape_segment(Some(encoded), None, None).is_none());
            assert!(glib::Uri::unescape_string(encoded, None).is_none());
        }
        assert_eq!(
            glib::Uri::unescape_string("%c3%a9", None).as_deref(),
            Some("é")
        );
        assert_eq!(glib::Uri::unescape_string("é", None).as_deref(), Some("é"));
    }
}
