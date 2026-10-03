/**
 * FreeRDP shadow subsystem that paints frames from Metis portal ScreenCast SHM.
 *
 * The Rust side writes BGRX pixels into a mmap'd file; this subsystem copies
 * them into the FreeRDP surface and calls shadow_subsystem_frame_update.
 */

#include "metis_shadow.h"

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <winpr/crt.h>
#include <winpr/synch.h>
#include <winpr/sysinfo.h>
#include <winpr/thread.h>

#include <freerdp/codec/color.h>
#include <freerdp/codec/region.h>
#include <freerdp/freerdp.h>
#include <freerdp/server/shadow.h>

typedef struct
{
	rdpShadowSubsystem common;
	HANDLE thread;
	HANDLE stopEvent;
	char* shmPath;
	char* inputPath;
	char* password;
	int shmFd;
	void* shmMap;
	size_t shmSize;
	uint32_t lastSeq;
	uint32_t width;
	uint32_t height;
} metisShadowSubsystem;

static metisShadowSubsystem* g_subsystem = NULL;
static rdpShadowServer* g_server = NULL;
static volatile int g_stop_requested = 0;

static void metis_send_input_json(metisShadowSubsystem* sub, const char* json)
{
	int fd = -1;
	struct sockaddr_un addr;

	if (!sub || !sub->inputPath || !json)
		return;

	fd = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
	if (fd < 0)
		return;

	memset(&addr, 0, sizeof(addr));
	addr.sun_family = AF_UNIX;
	strncpy(addr.sun_path, sub->inputPath, sizeof(addr.sun_path) - 1);
	(void)sendto(fd, json, strlen(json), MSG_NOSIGNAL, (struct sockaddr*)&addr, sizeof(addr));
	close(fd);
}

static BOOL metis_input_synchronize(rdpShadowSubsystem* subsystem, rdpShadowClient* client,
                                    UINT32 flags)
{
	(void)subsystem;
	(void)client;
	(void)flags;
	return TRUE;
}

static BOOL metis_input_keyboard(rdpShadowSubsystem* subsystem, rdpShadowClient* client,
                                 UINT16 flags, UINT8 code)
{
	char buf[160];
	int pressed = (flags & KBD_FLAGS_DOWN) ? 1 : 0;
	(void)client;
	/* Map RDP scancode roughly to Linux keycodes (scancode + 8). */
	UINT32 keycode = (UINT32)code + 8;
	snprintf(buf, sizeof(buf),
	         "{\"type\":\"key\",\"keycode\":%u,\"pressed\":%s}", keycode,
	         pressed ? "true" : "false");
	metis_send_input_json((metisShadowSubsystem*)subsystem, buf);
	return TRUE;
}

static BOOL metis_input_unicode(rdpShadowSubsystem* subsystem, rdpShadowClient* client, UINT16 flags,
                                UINT16 code)
{
	(void)subsystem;
	(void)client;
	(void)flags;
	(void)code;
	return TRUE;
}

static BOOL metis_input_mouse(rdpShadowSubsystem* subsystem, rdpShadowClient* client, UINT16 flags,
                              UINT16 x, UINT16 y)
{
	char buf[256];
	metisShadowSubsystem* sub = (metisShadowSubsystem*)subsystem;
	(void)client;

	if (flags & PTR_FLAGS_MOVE)
	{
		snprintf(buf, sizeof(buf), "{\"type\":\"abs\",\"x\":%u,\"y\":%u}", (unsigned)x,
		         (unsigned)y);
		metis_send_input_json(sub, buf);
	}

	if (flags & PTR_FLAGS_BUTTON1)
	{
		int pressed = (flags & PTR_FLAGS_DOWN) ? 1 : 0;
		snprintf(buf, sizeof(buf),
		         "{\"type\":\"button\",\"button\":272,\"pressed\":%s}",
		         pressed ? "true" : "false");
		metis_send_input_json(sub, buf);
	}
	else if (flags & PTR_FLAGS_BUTTON2)
	{
		int pressed = (flags & PTR_FLAGS_DOWN) ? 1 : 0;
		snprintf(buf, sizeof(buf),
		         "{\"type\":\"button\",\"button\":273,\"pressed\":%s}",
		         pressed ? "true" : "false");
		metis_send_input_json(sub, buf);
	}
	else if (flags & PTR_FLAGS_BUTTON3)
	{
		int pressed = (flags & PTR_FLAGS_DOWN) ? 1 : 0;
		snprintf(buf, sizeof(buf),
		         "{\"type\":\"button\",\"button\":274,\"pressed\":%s}",
		         pressed ? "true" : "false");
		metis_send_input_json(sub, buf);
	}

	if (flags & PTR_FLAGS_WHEEL)
	{
		int delta = (int)(flags & WheelRotationMask);
		if (flags & PTR_FLAGS_WHEEL_NEGATIVE)
			delta = -delta;
		snprintf(buf, sizeof(buf), "{\"type\":\"scroll\",\"dx\":0,\"dy\":%d}", delta);
		metis_send_input_json(sub, buf);
	}

	return TRUE;
}

static BOOL metis_input_ext_mouse(rdpShadowSubsystem* subsystem, rdpShadowClient* client,
                                  UINT16 flags, UINT16 x, UINT16 y)
{
	return metis_input_mouse(subsystem, client, flags, x, y);
}

static BOOL metis_input_rel_mouse(rdpShadowSubsystem* subsystem, rdpShadowClient* client,
                                  UINT16 flags, INT16 dx, INT16 dy)
{
	char buf[160];
	(void)client;
	(void)flags;
	snprintf(buf, sizeof(buf), "{\"type\":\"rel\",\"dx\":%d,\"dy\":%d}", (int)dx, (int)dy);
	metis_send_input_json((metisShadowSubsystem*)subsystem, buf);
	return TRUE;
}

static int metis_authenticate(rdpShadowSubsystem* subsystem, rdpShadowClient* client,
                              const char* user, const char* domain, const char* password)
{
	metisShadowSubsystem* sub = (metisShadowSubsystem*)subsystem;
	(void)client;
	(void)user;
	(void)domain;
	if (!sub->password || sub->password[0] == '\0')
		return 1; /* no password configured → accept */
	if (password && strcmp(password, sub->password) == 0)
		return 1;
	return 0;
}

static UINT32 metis_enum_monitors(MONITOR_DEF* monitors, UINT32 maxMonitors)
{
	if (!monitors || maxMonitors < 1)
		return 0;
	monitors[0].left = 0;
	monitors[0].top = 0;
	monitors[0].right = 1920;
	monitors[0].bottom = 1080;
	monitors[0].flags = 1;
	if (g_subsystem && g_subsystem->width > 0 && g_subsystem->height > 0)
	{
		monitors[0].right = (INT32)g_subsystem->width;
		monitors[0].bottom = (INT32)g_subsystem->height;
	}
	return 1;
}

static int metis_map_shm(metisShadowSubsystem* sub)
{
	MetisFrameHeader hdr;
	ssize_t n;

	if (!sub->shmPath)
		return -1;
	sub->shmFd = open(sub->shmPath, O_RDONLY | O_CLOEXEC);
	if (sub->shmFd < 0)
		return -1;
	n = pread(sub->shmFd, &hdr, sizeof(hdr), 0);
	if (n != (ssize_t)sizeof(hdr) || hdr.magic != METIS_FRAME_MAGIC || hdr.width == 0 ||
	    hdr.height == 0 || hdr.stride == 0)
	{
		/* Not ready yet — map a default size; remap later if needed. */
		sub->width = 1920;
		sub->height = 1080;
		sub->shmSize = sizeof(MetisFrameHeader) + (size_t)sub->width * sub->height * 4;
	}
	else
	{
		sub->width = hdr.width;
		sub->height = hdr.height;
		sub->shmSize = sizeof(MetisFrameHeader) + (size_t)hdr.stride * hdr.height;
	}
	sub->shmMap = mmap(NULL, sub->shmSize, PROT_READ, MAP_SHARED, sub->shmFd, 0);
	if (sub->shmMap == MAP_FAILED)
	{
		sub->shmMap = NULL;
		close(sub->shmFd);
		sub->shmFd = -1;
		return -1;
	}
	return 0;
}

static void metis_unmap_shm(metisShadowSubsystem* sub)
{
	if (sub->shmMap)
	{
		munmap(sub->shmMap, sub->shmSize);
		sub->shmMap = NULL;
	}
	if (sub->shmFd >= 0)
	{
		close(sub->shmFd);
		sub->shmFd = -1;
	}
}

static DWORD WINAPI metis_capture_thread(LPVOID arg)
{
	metisShadowSubsystem* sub = (metisShadowSubsystem*)arg;
	rdpShadowServer* server = sub->common.server;

	while (WaitForSingleObject(sub->stopEvent, 16) == WAIT_TIMEOUT)
	{
		MetisFrameHeader* hdr;
		rdpShadowSurface* surface;
		RECTANGLE_16 full;
		UINT32 count;

		if (!sub->shmMap)
		{
			if (metis_map_shm(sub) < 0)
				continue;
		}

		hdr = (MetisFrameHeader*)sub->shmMap;
		if (hdr->magic != METIS_FRAME_MAGIC || !hdr->ready || hdr->seq == sub->lastSeq)
			continue;

		/* Remap if geometry grew beyond the mapped region. */
		{
			size_t need = sizeof(MetisFrameHeader) + (size_t)hdr->stride * hdr->height;
			if (need > sub->shmSize)
			{
				metis_unmap_shm(sub);
				if (metis_map_shm(sub) < 0)
					continue;
				hdr = (MetisFrameHeader*)sub->shmMap;
			}
		}

		surface = server->surface;
		if (!surface || !surface->data)
			continue;

		if (surface->width != hdr->width || surface->height != hdr->height)
		{
			/* Surface resize is owned by FreeRDP; skip until sizes match. */
			sub->width = hdr->width;
			sub->height = hdr->height;
		}

		EnterCriticalSection(&(surface->lock));
		{
			BYTE* src = ((BYTE*)sub->shmMap) + sizeof(MetisFrameHeader);
			UINT32 copy_h = hdr->height < surface->height ? hdr->height : surface->height;
			UINT32 copy_w = hdr->width < surface->width ? hdr->width : surface->width;
			freerdp_image_copy_no_overlap(surface->data, surface->format, surface->scanline, 0, 0,
			                              (int)copy_w, (int)copy_h, src, PIXEL_FORMAT_BGRX32,
			                              (int)hdr->stride, 0, 0, NULL, FREERDP_FLIP_NONE);
			full.left = 0;
			full.top = 0;
			full.right = (UINT16)copy_w;
			full.bottom = (UINT16)copy_h;
			region16_union_rect(&(surface->invalidRegion), &(surface->invalidRegion), &full);
		}
		LeaveCriticalSection(&(surface->lock));

		ArrayList_Lock(server->clients);
		count = (UINT32)ArrayList_Count(server->clients);
		if (count > 0)
			shadow_subsystem_frame_update(&sub->common);
		ArrayList_Unlock(server->clients);

		sub->lastSeq = hdr->seq;
	}
	return 0;
}

static rdpShadowSubsystem* metis_subsystem_new(void)
{
	metisShadowSubsystem* sub = calloc(1, sizeof(metisShadowSubsystem));
	if (!sub)
		return NULL;
	sub->shmFd = -1;
	sub->common.Authenticate = metis_authenticate;
	sub->common.SynchronizeEvent = metis_input_synchronize;
	sub->common.KeyboardEvent = metis_input_keyboard;
	sub->common.UnicodeKeyboardEvent = metis_input_unicode;
	sub->common.MouseEvent = metis_input_mouse;
	sub->common.ExtendedMouseEvent = metis_input_ext_mouse;
	sub->common.RelMouseEvent = metis_input_rel_mouse;
	g_subsystem = sub;
	return &sub->common;
}

static void metis_subsystem_free(rdpShadowSubsystem* subsystem)
{
	metisShadowSubsystem* sub = (metisShadowSubsystem*)subsystem;
	if (!sub)
		return;
	free(sub->shmPath);
	free(sub->inputPath);
	free(sub->password);
	free(sub);
	if (g_subsystem == sub)
		g_subsystem = NULL;
}

static int metis_subsystem_init(rdpShadowSubsystem* subsystem)
{
	metisShadowSubsystem* sub = (metisShadowSubsystem*)subsystem;
	MONITOR_DEF* monitor;
	const char* shm = getenv("METIS_RDP_SHM_PATH");
	const char* input = getenv("METIS_RDP_INPUT_PATH");
	const char* password = getenv("METIS_RDP_PASSWORD");

	if (shm)
		sub->shmPath = _strdup(shm);
	if (input)
		sub->inputPath = _strdup(input);
	if (password)
		sub->password = _strdup(password);

	sub->common.numMonitors = metis_enum_monitors(sub->common.monitors, 16);
	sub->common.captureFrameRate = 30;
	sub->common.selectedMonitor = 0;
	monitor = &sub->common.monitors[0];
	sub->common.virtualScreen.left = monitor->left;
	sub->common.virtualScreen.top = monitor->top;
	sub->common.virtualScreen.right = monitor->right;
	sub->common.virtualScreen.bottom = monitor->bottom;
	sub->common.virtualScreen.flags = 1;
	sub->width = (UINT32)(monitor->right - monitor->left);
	sub->height = (UINT32)(monitor->bottom - monitor->top);
	(void)metis_map_shm(sub);
	return 1;
}

static int metis_subsystem_uninit(rdpShadowSubsystem* subsystem)
{
	metis_unmap_shm((metisShadowSubsystem*)subsystem);
	return 1;
}

static int metis_subsystem_start(rdpShadowSubsystem* subsystem)
{
	metisShadowSubsystem* sub = (metisShadowSubsystem*)subsystem;
	sub->stopEvent = CreateEvent(NULL, TRUE, FALSE, NULL);
	if (!sub->stopEvent)
		return -1;
	sub->thread = CreateThread(NULL, 0, metis_capture_thread, sub, 0, NULL);
	if (!sub->thread)
		return -1;
	return 1;
}

static int metis_subsystem_stop(rdpShadowSubsystem* subsystem)
{
	metisShadowSubsystem* sub = (metisShadowSubsystem*)subsystem;
	if (sub->stopEvent)
		SetEvent(sub->stopEvent);
	if (sub->thread)
	{
		WaitForSingleObject(sub->thread, INFINITE);
		CloseHandle(sub->thread);
		sub->thread = NULL;
	}
	if (sub->stopEvent)
	{
		CloseHandle(sub->stopEvent);
		sub->stopEvent = NULL;
	}
	return 1;
}

static int metis_subsystem_entry(RDP_SHADOW_ENTRY_POINTS* ep)
{
	ep->New = metis_subsystem_new;
	ep->Free = metis_subsystem_free;
	ep->Init = metis_subsystem_init;
	ep->Uninit = metis_subsystem_uninit;
	ep->Start = metis_subsystem_start;
	ep->Stop = metis_subsystem_stop;
	ep->EnumMonitors = metis_enum_monitors;
	return 1;
}

int metis_rdp_server_run(const char* shm_path, const char* input_path, uint16_t port,
                         const char* password)
{
	int status;

	g_stop_requested = 0;
	shadow_subsystem_set_entry(metis_subsystem_entry);

	g_server = shadow_server_new();
	if (!g_server)
		return 1;

	g_server->port = port;
	g_server->mayView = TRUE;
	g_server->mayInteract = TRUE;
	g_server->authentication = (password && password[0] != '\0') ? TRUE : FALSE;
	g_server->ShowMouseCursor = TRUE;

	/* Subsystem Init reads these (shadow_server_init creates the subsystem). */
	if (shm_path)
		setenv("METIS_RDP_SHM_PATH", shm_path, 1);
	if (input_path)
		setenv("METIS_RDP_INPUT_PATH", input_path, 1);
	if (password)
		setenv("METIS_RDP_PASSWORD", password, 1);
	else
		unsetenv("METIS_RDP_PASSWORD");

	status = shadow_server_init(g_server);
	if (status < 0)
	{
		shadow_server_free(g_server);
		g_server = NULL;
		return 2;
	}

	status = shadow_server_start(g_server);
	if (status < 0)
	{
		shadow_server_uninit(g_server);
		shadow_server_free(g_server);
		g_server = NULL;
		return 3;
	}

	while (!g_stop_requested)
		Sleep(200);

	shadow_server_stop(g_server);
	shadow_server_uninit(g_server);
	shadow_server_free(g_server);
	g_server = NULL;
	return 0;
}

void metis_rdp_server_stop(void)
{
	g_stop_requested = 1;
}
